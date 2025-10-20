#![cfg_attr(not(feature = "std"), no_std)]
#![deny(unsafe_code)]
#![doc = include_str!("../README.md")]

pub mod iter;
pub mod ops;
pub mod range;
#[cfg(feature = "alloc")]
pub mod set;
#[cfg(test)]
pub(crate) mod test;

/// A prelude for the crate.
pub mod prelude {
    #[cfg(feature = "alloc")]
    pub use crate::set::RangeSet;
    pub use crate::{iter::RangeIterator, ops::*};
}

/// A type which successor and predecessor operations can be performed on.
///
/// Similar to `std::iter::Step`, but not nightly-only.
pub trait Step: Sized {
    /// Steps forwards by `count` elements.
    fn forward(start: Self, count: usize) -> Option<Self>;

    /// Steps backwards by `count` elements.
    fn backward(start: Self, count: usize) -> Option<Self>;
}

macro_rules! impl_step {
    ($($ty:ty),+) => {
        $(
            impl Step for $ty {
                fn forward(start: Self, count: usize) -> Option<Self> {
                    start.checked_add(count as Self)
                }

                fn backward(start: Self, count: usize) -> Option<Self> {
                    start.checked_sub(count as Self)
                }
            }
        )*
    };
}

impl_step!(
    u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize
);
