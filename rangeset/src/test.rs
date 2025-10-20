use core::{
    fmt::{Debug, Display},
    ops::{BitAnd, BitOr, BitXor, Range, Sub},
};

use crate::iter::{IntoRangeIterator, RangeIterator};

pub const TEST_DOMAIN_SIZE: usize = 10;

/// An extension trait for testing.
pub trait RangeIteratorAssertExt<T>: RangeIterator<T> {
    fn checked(self) -> Checked<T, Self>
    where
        Self: Sized,
    {
        Checked {
            iter: self,
            pos: 0,
            prev: None,
        }
    }
}

impl<T, I: RangeIterator<T>> RangeIteratorAssertExt<T> for I {}

/// Asserts equality of an operation on all pairs of sets of the universe.
pub fn assert_pairwise<
    I0: IntoRangeIterator<usize>,
    I1: IntoRangeIterator<usize>,
    I2: IntoRangeIterator<usize>,
>(
    domain: usize,
    make: impl Fn(Set, Set) -> (I0, I1),
    op_0: impl Fn(Set, Set) -> Set,
    op_1: impl Fn(I0, I1) -> I2,
) {
    for a_ref in Universe::new(domain).iter_sets() {
        for b_ref in Universe::new(domain).iter_sets() {
            let expected = op_0(a_ref, b_ref);
            let (a, b) = make(a_ref, b_ref);
            op_1(a, b)
                .into_range_iter()
                .checked()
                .zip(expected.iter_ranges())
                .enumerate()
                .for_each(|(i, (a, b))| {
                    assert_eq!(a, b, "f({a_ref},{b_ref}) != {expected} at index {i}")
                });
        }
    }
}

/// Asserts a predicate of an operation on all pairs of sets of the universe.
pub fn assert_pairwise_bool<I0: IntoRangeIterator<usize>, I1: IntoRangeIterator<usize>>(
    domain: usize,
    make: impl Fn(Set, Set) -> (I0, I1),
    op_0: impl Fn(Set, Set) -> bool,
    op_1: impl Fn(I0, I1) -> bool,
) {
    for a_ref in Universe::new(domain).iter_sets() {
        for b_ref in Universe::new(domain).iter_sets() {
            let expected = op_0(a_ref, b_ref);
            let (a, b) = make(a_ref, b_ref);
            assert_eq!(op_1(a, b), expected, "f({a_ref},{b_ref}) != {expected}");
        }
    }
}

/// Asserts equality of an operation on all pairs of ranges of the universe.
pub fn assert_pairwise_ranges<I: IntoRangeIterator<usize>>(
    domain: usize,
    op_0: impl Fn(Set, Set) -> Set,
    op_1: impl Fn(Range<usize>, Range<usize>) -> I,
) {
    for a in Universe::new(domain).iter_ranges() {
        for b in Universe::new(domain).iter_ranges() {
            let expected = op_0(a, b);
            let a = a.into_range().unwrap();
            let b = b.into_range().unwrap();
            op_1(a, b)
                .into_range_iter()
                .checked()
                .zip(expected.iter_ranges())
                .enumerate()
                .for_each(|(i, (a, b))| assert_eq!(a, b, "at index {i}"));
        }
    }
}

/// Iterator which asserts invariants of the underlying iterator.
#[derive(Debug)]
pub struct Checked<T, I> {
    iter: I,
    pos: usize,
    prev: Option<Range<T>>,
}

impl<T: Copy + Ord + Debug, I: Iterator<Item = Range<T>>> Iterator for Checked<T, I> {
    type Item = Range<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if let Some(range) = self.iter.next() {
            let i = self.pos;
            self.pos += 1;

            assert!(
                range.start <= range.end,
                "range is empty ({i}): {:?}",
                range
            );

            if let Some(prev) = self.prev.take() {
                assert!(
                    range.end > prev.end,
                    "ranges are not sorted ({i}): {:?} {:?}",
                    prev,
                    range
                );
                assert!(
                    range.start > prev.start,
                    "ranges are not sorted ({i}): {:?} {:?}",
                    prev,
                    range
                );
                assert!(
                    range.start > prev.end,
                    "ranges are adjacent ({i}): {:?} {:?}",
                    prev,
                    range
                );
            }

            self.prev = Some(range.clone());

            Some(range)
        } else {
            assert!(self.iter.next().is_none(), "iter is not exhausted");
            None
        }
    }
}

impl<T: Copy + Ord + Debug, I: Iterator<Item = Range<T>>> RangeIterator<T> for Checked<T, I> {}

impl<T: Copy + Ord + Debug, I: Iterator<Item = Range<T>>> IntoRangeIterator<T> for Checked<T, I> {
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// The universe of sets in a domain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Universe(u64);

impl Universe {
    /// Creates a new universe.
    ///
    /// # Panics
    ///
    /// Panics if `size > 64`.
    pub fn new(size: usize) -> Self {
        assert!(size < 64, "size must be < 64");
        Self(size as u64)
    }

    /// Returns the full set.
    pub fn full(&self) -> Set {
        Set::new((1 << self.0) - 1)
    }

    /// Returns an iterator over all contiguous ranges in the universe.
    pub fn iter_ranges(&self) -> impl Iterator<Item = Set> {
        core::iter::once(Set::new(0)).chain((0..self.0).flat_map(move |start| {
            let width = self.0 - start;
            (1..=width).scan(0, move |m, len| {
                let bit = 1 << (start + len - 1);
                *m |= bit;
                Some(Set::new(*m))
            })
        }))
    }

    /// Returns an iterator over all sets of the universe.
    pub fn iter_sets(&self) -> impl Iterator<Item = Set> {
        (0..(1 << self.0)).map(Set::new)
    }
}

/// Set of values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Set {
    mask: u64,
}

impl Set {
    /// Creates a new set from a bit mask.
    pub fn new(mask: u64) -> Self {
        Self { mask }
    }

    /// Returns `true` if the set is empty.
    pub fn is_empty(&self) -> bool {
        self.mask == 0
    }

    /// Returns the maximum value in the set, or `None` if the set is empty.
    pub fn max(&self) -> Option<usize> {
        if self.mask != 0 {
            Some(63 - self.mask.leading_zeros() as usize)
        } else {
            None
        }
    }

    /// Shifts the set left by `n`.
    pub fn shift_left(&mut self, n: usize) {
        self.mask >>= n;
    }

    /// Shifts the set right by `n`.
    pub fn shift_right(&mut self, n: usize) {
        self.mask <<= n;
    }

    /// Returns an iterator over the ranges in the set.
    pub fn iter_ranges(&self) -> RangeIter {
        RangeIter::new(self.mask)
    }

    /// Returns the contiguous range in the set, returning `None` if the
    /// set is not contiguous.
    pub fn into_range(self) -> Option<Range<usize>> {
        let m = self.mask;
        if m == 0 {
            return Some(0..0);
        }

        let tz = m.trailing_zeros() as usize;
        let s = m >> tz;
        if s & (s + 1) != 0 {
            // Not contiguous.
            return None;
        }
        let len = s.count_ones() as usize;
        Some(tz..tz + len)
    }
}

impl Display for Set {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let len = self.iter_ranges().count();
        f.write_str("[")?;
        for (i, range) in self.iter_ranges().enumerate() {
            f.write_str(&format!("{:?}", range))?;
            if i < len - 1 {
                f.write_str(", ")?;
            }
        }
        f.write_str("]")
    }
}

impl BitXor<Set> for Set {
    type Output = Set;

    fn bitxor(self, rhs: Set) -> Self::Output {
        Set {
            mask: self.mask ^ rhs.mask,
        }
    }
}

impl BitAnd<Set> for Set {
    type Output = Set;

    fn bitand(self, rhs: Set) -> Self::Output {
        Set {
            mask: self.mask & rhs.mask,
        }
    }
}

impl BitOr<Set> for Set {
    type Output = Set;

    fn bitor(self, rhs: Set) -> Self::Output {
        Set {
            mask: self.mask | rhs.mask,
        }
    }
}

impl Sub<Set> for Set {
    type Output = Set;

    fn sub(self, rhs: Set) -> Self::Output {
        Set {
            mask: self.mask & !rhs.mask,
        }
    }
}

impl IntoRangeIterator<usize> for Set {
    type IntoIter = RangeIter;

    fn into_range_iter(self) -> Self::IntoIter {
        self.iter_ranges()
    }
}

/// Iterates over runs of 1-bits in `bits` as half-open ranges [start, end).
/// Example: bits `..0011_1010` => yields [1,2), [3,5), [6,7)
#[derive(Clone, Debug)]
pub struct RangeIter {
    bits: u64,
    idx: usize,
}

impl RangeIter {
    #[inline]
    pub fn new(mask: u64) -> Self {
        Self { bits: mask, idx: 0 }
    }
}

impl Iterator for RangeIter {
    type Item = Range<usize>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let mut m = self.bits;
        if m == 0 {
            return None;
        }

        // Skip 0s to the next run of 1s.
        let tz = m.trailing_zeros() as usize;
        m >>= tz;
        self.idx += tz;

        // Length of this 1-run = trailing_zeros(!m) (since LSB is 1 now).
        let run = (!m).trailing_zeros() as usize;

        // Emit [idx, idx + run)
        let start = self.idx;
        let end = start + run;

        // Consume the run.
        m >>= run;
        self.bits = m;
        self.idx = end;

        Some(start..end)
    }
}

impl RangeIterator<usize> for RangeIter {}

impl IntoRangeIterator<usize> for RangeIter {
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mask_range_iter() {
        for set in Universe::new(TEST_DOMAIN_SIZE).iter_sets() {
            set.into_range_iter().checked().for_each(drop);
        }
    }

    #[test]
    fn test_universe_iter_ranges() {
        let domain = 4;
        let universe = Universe::new(domain);
        assert_eq!(
            universe.iter_ranges().count(),
            (domain * (domain + 1) / 2) + 1,
        );
    }

    #[test]
    fn test_set_max() {
        for i in 0..TEST_DOMAIN_SIZE {
            let set = Set::new(1 << i);
            if set.is_empty() {
                assert_eq!(set.max(), None);
            } else {
                assert_eq!(set.max(), Some(i));
            }
        }
    }
}
