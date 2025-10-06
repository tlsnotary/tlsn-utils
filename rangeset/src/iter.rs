//! Iterator types.

use core::ops::Range;

#[cfg(feature = "alloc")]
use crate::set::RangeSet;

/// A range iterator.
///
/// Implementations of this trait *must* ensure the following invariants:
///
/// - The ranges are sorted.
/// - The ranges are non-empty.
/// - The ranges are non-adjacent.
/// - The ranges are disjoint.
///
/// Breaking these invariants will produce undefined meaningless behavior.
pub trait RangeIterator<T>: Iterator<Item = Range<T>> {
    /// Seek to the next range which contains the value `n`.
    ///
    /// This advances the iterator until it yields a range which contains `n`.
    ///
    /// If `n` is not present in the iterator it will yield `None`. Afterwards,
    /// the iterator may still yield ranges with values greater than `n`.
    fn seek(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        self.seek_end_ge(n)
            .and_then(|range| (range.contains(n)).then_some(range))
    }

    /// Seek to the next range where `range.end >= n`.
    ///
    /// This advances the iterator until it yields a range where `range.end >=
    /// n`.
    ///
    /// If no such range exists, the iterator will yield `None`. Afterwards,
    /// the iterator may still yield ranges past `n`.
    ///
    /// # Implementors
    ///
    /// Slice-backed iterators can override this method to improve performance
    /// using binary search.
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        while let Some(range) = self.next() {
            if &range.end >= n {
                return Some(range);
            }
        }
        None
    }

    /// Returns an iterator which yields the union of `self` and `other`.
    fn union<I>(self, other: I) -> UnionIter<T, Self, I::IntoIter>
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
    {
        UnionIter::new(self, other.into_range_iter())
    }

    /// Returns an iterator which yields the difference of `self` and `other`.
    fn difference<I>(self, other: I) -> DifferenceIter<T, Self, I::IntoIter>
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
    {
        DifferenceIter::new(self, other.into_range_iter())
    }

    /// Returns an iterator which yields the symmetric difference of `self` and
    /// `other`.
    fn symmetric_difference<I>(self, other: I) -> SymmetricDifferenceIter<T, Self, I::IntoIter>
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
    {
        SymmetricDifferenceIter::new(self, other.into_range_iter())
    }

    /// Returns an iterator which yields the intersection of `self` and `other`.
    fn intersection<I>(self, other: I) -> IntersectionIter<T, Self, I::IntoIter>
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
    {
        IntersectionIter::new(self, other.into_range_iter())
    }

    /// Returns true if `self` is a subset of `other`.
    #[inline]
    fn is_subset<I>(mut self, other: I) -> bool
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
        T: Ord,
    {
        let mut b_iter = other.into_range_iter();
        let mut b: Option<Range<T>> = None;

        while let Some(a) = self.next() {
            if b.as_ref().map_or(true, |b| b.end < a.start) {
                b = b_iter.seek_end_ge(&a.start);
            }

            let Some(b) = b.as_ref() else { return false };
            if b.start > a.start || b.end < a.end {
                return false;
            }
        }

        true
    }

    /// Returns true if `self` is a superset of `other`.
    #[inline]
    fn is_superset<I>(mut self, other: I) -> bool
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
        T: Ord,
    {
        let mut a_iter = other.into_range_iter();
        let mut b: Option<Range<T>> = None;

        while let Some(a) = a_iter.next() {
            if b.as_ref().map_or(true, |b| b.end < a.start) {
                b = self.seek_end_ge(&a.start);
            }

            let Some(b) = b.as_ref() else { return false };
            if b.start > a.start || b.end < a.end {
                return false;
            }
        }

        true
    }

    /// Returns true if `self` is disjoint from `other`.
    #[inline]
    fn is_disjoint<I>(mut self, other: I) -> bool
    where
        Self: Sized,
        I: IntoRangeIterator<T>,
        T: Ord,
    {
        let mut b_iter = other.into_range_iter();
        let mut a = None;
        let mut b = None;

        loop {
            let (Some(aa), Some(bb)) = (
                a.take().or_else(|| self.next()),
                b.take().or_else(|| b_iter.next()),
            ) else {
                return true;
            };

            if aa.end <= bb.start {
                a = self.seek_end_ge(&bb.start);
                b = Some(bb);
            } else if bb.end <= aa.start {
                b = b_iter.seek_end_ge(&aa.start);
                a = Some(aa);
            } else {
                return false;
            }
        }
    }

    /// Creates a new [`RangeSet`] from the iterator.
    ///
    /// This method is more performant than [`RangeSet::from_iter`] because it
    /// does not need to perform checks.
    #[cfg(feature = "alloc")]
    fn into_set(self) -> RangeSet<T>
    where
        Self: Sized + IntoRangeIterator<T>,
    {
        RangeSet::from_range_iter(self)
    }
}

/// A type which can be created from a [`RangeIterator`].
pub trait FromRangeIterator<T>: Sized {
    /// Creates a value from a range iterator.
    fn from_range_iter<I>(iter: I) -> Self
    where
        I: IntoRangeIterator<T>;
}

/// A type which can be converted into a [`RangeIterator`].
pub trait IntoRangeIterator<T> {
    /// The iterator type.
    type IntoIter: RangeIterator<T>;

    /// Returns an iterator which yields the ranges of `self`.
    fn into_range_iter(self) -> Self::IntoIter;
}

/// Iterator returned by [`RangeIterator::union`].
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct UnionIter<T, A, B> {
    a_iter: A,
    b_iter: B,
    a: Option<Range<T>>,
    b: Option<Range<T>>,
}

impl<T, A, B> UnionIter<T, A, B> {
    #[inline]
    fn new(a: A, b: B) -> Self {
        Self {
            a_iter: a,
            b_iter: b,
            a: None,
            b: None,
        }
    }
}

impl<T, A, B> Iterator for UnionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match (
            self.a.take().or_else(|| self.a_iter.next()),
            self.b.take().or_else(|| self.b_iter.next()),
        ) {
            (Some(a), Some(b)) => {
                if a.end < b.start {
                    // `a` is entirely before `b`.
                    self.b = Some(b);
                    return Some(a);
                } else if b.end < a.start {
                    // `b` is entirely before `a`.
                    self.a = Some(a);
                    return Some(b);
                }

                // Keep merging ranges from both iterators until we reach a disjoint boundary or
                // they are exhausted.
                let mut current = a.start.min(b.start)..a.end.max(b.end);
                loop {
                    let mut extended = false;

                    if let Some(aa) = self
                        .a
                        .take()
                        .or_else(|| self.a_iter.seek_end_ge(&current.end))
                    {
                        if aa.start <= current.end {
                            if current.end < aa.end {
                                current.end = aa.end;
                            }
                            extended = true;
                        } else {
                            self.a = Some(aa);
                        }
                    }

                    if let Some(bb) = self
                        .b
                        .take()
                        .or_else(|| self.b_iter.seek_end_ge(&current.end))
                    {
                        if bb.start <= current.end {
                            if current.end < bb.end {
                                current.end = bb.end;
                            }
                            extended = true;
                        } else {
                            self.b = Some(bb);
                        }
                    }

                    if !extended {
                        break;
                    }
                }

                return Some(current);
            }
            (Some(x), None) | (None, Some(x)) => Some(x),
            (None, None) => None,
        }
    }
}

impl<T, A, B> RangeIterator<T> for UnionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        self.a = if let Some(a) = self.a.take()
            && &a.end >= n
        {
            Some(a)
        } else {
            self.a_iter.seek_end_ge(n)
        };

        self.b = if let Some(b) = self.b.take()
            && &b.end >= n
        {
            Some(b)
        } else {
            self.b_iter.seek_end_ge(n)
        };

        self.next()
    }
}

impl<T, A, B> IntoRangeIterator<T> for UnionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// Iterator returned by [`RangeIterator::difference`].
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct DifferenceIter<T, A, B> {
    this_iter: A,
    other_iter: B,
    this: Option<Range<T>>,
    other: Option<Range<T>>,
}

impl<T, A, B> DifferenceIter<T, A, B> {
    #[inline]
    fn new(a: A, b: B) -> Self {
        Self {
            this_iter: a,
            other_iter: b,
            this: None,
            other: None,
        }
    }
}

impl<T, A, B> Iterator for DifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let this = self.this.take().or_else(|| self.this_iter.next())?;

            let Some(other) = self
                .other
                .take()
                .or_else(|| self.other_iter.seek_end_ge(&this.start))
            else {
                return Some(this);
            };

            if this.end <= other.start {
                // This range is entirely before the other range.
                self.other = Some(other);
                return Some(this);
            } else if this.start >= other.end {
                // This range is entirely after the other range.
                self.other = self.other_iter.seek_end_ge(&this.start);
                self.this = Some(this);
                continue;
            } else if this.start < other.start {
                // This range precedes the other range.
                let left = this.start..other.start;

                // Store the remainder.
                let rem_start = this.start.max(other.end);
                if rem_start < this.end {
                    self.this = Some(rem_start..this.end);
                }

                self.other = Some(other);

                return Some(left);
            } else {
                // The other range precedes this range.
                let right_start = this.start.max(other.end);
                if right_start < this.end {
                    self.this = Some(right_start..this.end);
                }

                self.other = Some(other);

                continue;
            }
        }
    }
}

impl<T, A, B> RangeIterator<T> for DifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        // If we have a buffered `this` that ends before `n`, drop it.
        if let Some(t) = self.this.take() {
            if &t.end >= n {
                self.this = Some(t);
            }
        }

        // Ensure `this` is positioned at the first range with end >= n.
        if self.this.is_none() {
            self.this = self.this_iter.seek_end_ge(n);
            if self.this.is_none() {
                return None;
            }
        }

        // Iterate until we find one with end >= n.
        while let Some(out) = self.next() {
            if &out.end >= n {
                return Some(out);
            }
        }

        None
    }
}

impl<T, A, B> IntoRangeIterator<T> for DifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// Iterator returned by [`RangeIterator::intersection`].
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct IntersectionIter<T, A, B> {
    a_iter: A,
    b_iter: B,
    a: Option<Range<T>>,
    b: Option<Range<T>>,
}

impl<T, A, B> IntersectionIter<T, A, B> {
    #[inline]
    fn new(a: A, b: B) -> Self {
        Self {
            a_iter: a,
            b_iter: b,
            a: None,
            b: None,
        }
    }
}

impl<T, A, B> Iterator for IntersectionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let a = self.a.take().or_else(|| self.a_iter.next())?;
            let b = self.b.take().or_else(|| self.b_iter.next())?;

            if a.end <= b.start {
                // `a` is entirely before `b`.
                self.a = self.a_iter.seek_end_ge(&b.start);
                self.b = Some(b);
                continue;
            }

            if b.end <= a.start {
                // `b` is entirely before `a`.
                self.b = self.b_iter.seek_end_ge(&a.start);
                self.a = Some(a);
                continue;
            }

            let current = a.start.max(b.start)..a.end.min(b.end);

            if a.end <= b.end {
                self.b = Some(b);
            } else {
                self.a = Some(a);
            }

            return Some(current);
        }
    }
}

impl<T, A, B> RangeIterator<T> for IntersectionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    #[inline]
    fn seek_end_ge(&mut self, n: &T) -> Option<Range<T>>
    where
        T: Ord,
    {
        self.a = if let Some(a) = self.a.take()
            && &a.end >= n
        {
            Some(a)
        } else {
            self.a_iter.seek_end_ge(n)
        };

        self.b = if let Some(b) = self.b.take()
            && &b.end >= n
        {
            Some(b)
        } else {
            self.b_iter.seek_end_ge(n)
        };

        self.next()
    }
}

impl<T, A, B> IntoRangeIterator<T> for IntersectionIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

/// Iterator returned by [`RangeIterator::symmetric_difference`].
#[derive(Debug)]
#[must_use = "iterators are lazy and do nothing unless consumed"]
pub struct SymmetricDifferenceIter<T, A, B> {
    a_iter: A,
    b_iter: B,
    a: Option<Range<T>>,
    b: Option<Range<T>>,
    current: Option<Range<T>>,
}

impl<T, A, B> SymmetricDifferenceIter<T, A, B> {
    #[inline]
    pub fn new(a: A, b: B) -> Self {
        Self {
            a_iter: a,
            b_iter: b,
            a: None,
            b: None,
            current: None,
        }
    }
}

impl<T, A, B> SymmetricDifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Ord,
{
    #[inline]
    fn emit_or_coalesce(&mut self, chunk: Range<T>) -> Option<Range<T>> {
        if let Some(current) = self.current.take() {
            if current.end == chunk.start {
                self.current = Some(current.start..chunk.end);
                None
            } else {
                self.current = Some(chunk);
                Some(current)
            }
        } else {
            self.current = Some(chunk);
            None
        }
    }

    #[inline]
    fn next_chunks(&mut self) -> (Option<Range<T>>, Option<Range<T>>) {
        if let Some(current) = self.current.as_ref() {
            (
                self.a
                    .take()
                    .or_else(|| self.a_iter.seek_end_ge(&current.end)),
                self.b
                    .take()
                    .or_else(|| self.b_iter.seek_end_ge(&current.end)),
            )
        } else {
            (
                self.a.take().or_else(|| self.a_iter.next()),
                self.b.take().or_else(|| self.b_iter.next()),
            )
        }
    }
}

impl<T, A, B> Iterator for SymmetricDifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type Item = Range<T>;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.next_chunks() {
                (Some(a), Some(b)) => {
                    let next = if a.end <= b.start {
                        self.b = Some(b);
                        a
                    } else if b.end <= a.start {
                        self.a = Some(a);
                        b
                    } else if a.start < b.start {
                        let chunk = a.start..b.start;
                        self.a = Some(b.start..a.end);
                        self.b = Some(b);
                        chunk
                    } else if b.start < a.start {
                        let chunk = b.start..a.start;
                        self.b = Some(a.start..b.end);
                        self.a = Some(a);
                        chunk
                    } else {
                        // a.start == b.start.
                        let right_start = a.end.min(b.end);
                        if a.end == right_start {
                            if right_start < b.end {
                                self.b = Some(right_start..b.end);
                            }
                        } else {
                            if right_start < a.end {
                                self.a = Some(right_start..a.end);
                            }
                        }
                        continue;
                    };

                    if let Some(emit) = self.emit_or_coalesce(next) {
                        return Some(emit);
                    }
                }
                (Some(x), None) | (None, Some(x)) => {
                    if let Some(emit) = self.emit_or_coalesce(x) {
                        return Some(emit);
                    }
                }
                (None, None) => return self.current.take(),
            }
        }
    }
}

impl<T, A, B> RangeIterator<T> for SymmetricDifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
}

impl<T, A, B> IntoRangeIterator<T> for SymmetricDifferenceIter<T, A, B>
where
    A: RangeIterator<T>,
    B: RangeIterator<T>,
    T: Copy + Ord,
{
    type IntoIter = Self;

    fn into_range_iter(self) -> Self::IntoIter {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::{Set, TEST_DOMAIN_SIZE, assert_pairwise, assert_pairwise_bool};

    #[test]
    fn test_union_iter() {
        assert_pairwise(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| a | b,
            |a, b| a.into_range_iter().union(b),
        );
    }

    #[test]
    fn test_difference_iter() {
        assert_pairwise(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| a - b,
            |a, b| a.into_range_iter().difference(b),
        );
    }

    #[test]
    fn test_intersection_iter() {
        assert_pairwise(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| a & b,
            |a, b| a.into_range_iter().intersection(b),
        );
    }

    #[test]
    fn test_symmetric_difference_iter() {
        assert_pairwise(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| a ^ b,
            |a, b| a.into_range_iter().symmetric_difference(b),
        );
    }

    #[test]
    fn test_is_subset() {
        assert_pairwise_bool(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| (a & b) == a,
            |a, b| a.into_range_iter().is_subset(b.into_range_iter()),
        );
    }

    #[test]
    fn test_is_superset() {
        assert_pairwise_bool(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| (a & b) == b,
            |a, b| a.into_range_iter().is_superset(b.into_range_iter()),
        );
    }

    #[test]
    fn test_is_disjoint() {
        assert_pairwise_bool(
            TEST_DOMAIN_SIZE,
            |a, b| (a, b),
            |a, b| (a & b) == Set::new(0),
            |a, b| a.into_range_iter().is_disjoint(b.into_range_iter()),
        );
    }
}
