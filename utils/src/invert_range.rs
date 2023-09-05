use std::ops::Range;
use thiserror::Error;

/// Inverts a range depending on a set of ranges, i.e. returns the complement
///
/// This function takes a range and a set of ranges to remove from the original range. For example
/// if you provide the `original_range` `1..10` and the `ranges_to_remove` `2..5` and `7..8` the
/// function will return the ranges `1..2`, `5..7` and `8..10`.
///
/// Every range in `ranges_to_remove` must be contained in `original_range`.
/// Ranges must not overlap and cannot be empty or negative.
pub fn invert_range<T: Ord + Copy>(
    original_range: &Range<T>,
    ranges_to_remove: &[Range<T>],
) -> Result<Vec<Range<T>>, RangeError> {
    // Check that original_range is valid
    if original_range.start >= original_range.end {
        return Err(RangeError::Invalid);
    }

    for (k, range) in ranges_to_remove.iter().enumerate() {
        // Check that there is no invalid or empty range
        if range.start >= range.end {
            return Err(RangeError::Invalid);
        }

        // Check that ranges are not out of bounds
        if range.start >= original_range.end
            || range.end > original_range.end
            || range.start < original_range.start
            || range.end <= original_range.start
        {
            return Err(RangeError::OutOfBounds);
        }

        // Check that ranges are not overlapping
        if ranges_to_remove
            .iter()
            .enumerate()
            .any(|(l, r)| k != l && r.start < range.end && r.end > range.start)
        {
            return Err(RangeError::Overlapping);
        }
    }

    // Now invert ranges
    let mut inverted = vec![original_range.clone()];

    for range in ranges_to_remove.iter() {
        let inv = inverted
            .iter_mut()
            .find(|inv| range.start >= inv.start && range.end <= inv.end)
            .expect("Should have found range to invert");

        let original_end = inv.end;
        inv.end = range.start;

        inverted.push(Range {
            start: range.end,
            end: original_end,
        });
    }

    // Remove empty ranges
    inverted.retain(|r| r.start != r.end);

    Ok(inverted)
}

/// Errors that can occur during range manipulation
#[allow(missing_docs)]
#[derive(Debug, Error)]
pub enum RangeError {
    #[error("Found zero or negative range")]
    Invalid,
    #[error("Found out of bounds range")]
    OutOfBounds,
    #[error("Found overlapping ranges")]
    Overlapping,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invert_ranges_errors() {
        let empty_range = Range { start: 0, end: 0 };
        let invalid_range = Range { start: 2, end: 1 };
        let out_of_bounds = Range { start: 4, end: 11 };

        let ranges = vec![empty_range, invalid_range, out_of_bounds];

        for range in ranges {
            assert!(invert_range(
                &Range {
                    start: 0_usize,
                    end: 10
                },
                &[range]
            )
            .is_err());
        }
    }

    #[test]
    fn test_invert_ranges_overlapping() {
        let overlapping1 = vec![Range { start: 2, end: 5 }, Range { start: 4, end: 7 }];
        let overlapping2 = vec![Range { start: 2, end: 5 }, Range { start: 1, end: 4 }];
        let overlapping3 = vec![Range { start: 2, end: 5 }, Range { start: 3, end: 4 }];
        let overlapping4 = vec![Range { start: 2, end: 5 }, Range { start: 2, end: 5 }];

        // this should not be an error
        let ok1 = vec![Range { start: 2, end: 5 }, Range { start: 5, end: 8 }];
        let ok2 = vec![Range { start: 2, end: 5 }, Range { start: 7, end: 10 }];

        let overlap = vec![overlapping1, overlapping2, overlapping3, overlapping4];
        let ok = vec![ok1, ok2];

        for range in overlap {
            assert!(invert_range(
                &Range {
                    start: 1_usize,
                    end: 10
                },
                &range,
            )
            .is_err());
        }

        for range in ok {
            assert!(invert_range(
                &Range {
                    start: 1_usize,
                    end: 10
                },
                &range
            )
            .is_ok());
        }
    }

    #[test]
    fn test_invert_ranges() {
        let ranges = &[
            Range { start: 1, end: 5 },
            Range { start: 5, end: 10 },
            Range { start: 12, end: 16 },
            Range { start: 18, end: 20 },
        ];

        let expected = vec![Range { start: 10, end: 12 }, Range { start: 16, end: 18 }];

        assert_eq!(
            invert_range(
                &Range {
                    start: 1_usize,
                    end: 20
                },
                ranges,
            )
            .unwrap(),
            expected
        );
    }
}
