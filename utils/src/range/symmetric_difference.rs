use std::ops::{BitXor, BitXorAssign, Range};

use crate::range::{DifferenceMut, Intersection, RangeSet, UnionMut};

pub trait SymmetricDifferenceMut<Rhs> {
    /// Replaces `self` with the set symmetric difference of `self` and `other`.
    fn symmetric_difference_mut(&mut self, other: &Rhs);
}

pub trait SymmetricDifference<Rhs> {
    type Output;

    /// Returns the set symmetric difference of `self` and `other`.
    fn symmetric_difference(&self, other: &Rhs) -> Self::Output;
}

impl<T: Copy + Ord> SymmetricDifferenceMut<Range<T>> for RangeSet<T> {
    fn symmetric_difference_mut(&mut self, other: &Range<T>) {
        let intersection = self.intersection(other);
        self.union_mut(other);
        self.difference_mut(&intersection);
    }
}

impl<T: Copy + Ord> SymmetricDifferenceMut<RangeSet<T>> for RangeSet<T> {
    fn symmetric_difference_mut(&mut self, other: &RangeSet<T>) {
        let intersection = self.intersection(other);
        self.union_mut(other);
        self.difference_mut(&intersection);
    }
}

impl<T: Copy + Ord> SymmetricDifference<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn symmetric_difference(&self, other: &Range<T>) -> Self::Output {
        let mut output = self.clone();
        output.symmetric_difference_mut(other);
        output
    }
}

impl<T: Copy + Ord> SymmetricDifference<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn symmetric_difference(&self, other: &RangeSet<T>) -> Self::Output {
        let mut output = self.clone();
        output.symmetric_difference_mut(other);
        output
    }
}

impl<T: Copy + Ord> BitXor<Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: Range<T>) -> Self::Output {
        self.symmetric_difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<&Range<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: &Range<T>) -> Self::Output {
        self.symmetric_difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: RangeSet<T>) -> Self::Output {
        self.symmetric_difference_mut(&rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<&RangeSet<T>> for RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(mut self, rhs: &RangeSet<T>) -> Self::Output {
        self.symmetric_difference_mut(rhs);
        self
    }
}

impl<T: Copy + Ord> BitXor<RangeSet<T>> for &RangeSet<T> {
    type Output = RangeSet<T>;

    fn bitxor(self, rhs: RangeSet<T>) -> Self::Output {
        self.symmetric_difference(&rhs)
    }
}

impl<T: Copy + Ord> BitXorAssign<RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: RangeSet<T>) {
        self.symmetric_difference_mut(&rhs);
    }
}

impl<T: Copy + Ord> BitXorAssign<&RangeSet<T>> for RangeSet<T> {
    fn bitxor_assign(&mut self, rhs: &RangeSet<T>) {
        self.symmetric_difference_mut(rhs);
    }
}
