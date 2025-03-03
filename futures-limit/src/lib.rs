#![doc = include_str!("../README.md")]

pub(crate) mod bucket;
mod delay;
mod rate;

pub use delay::{Delay, DelayFuture};
pub use rate::Rate;

use futures::{AsyncRead, AsyncWrite};
#[cfg(test)]
pub(crate) use mock_instant::thread_local::Instant;
#[cfg(all(not(test), not(target_arch = "wasm32")))]
pub(crate) use std::time::Instant;
#[cfg(all(not(test), target_arch = "wasm32"))]
pub(crate) use web_time::Instant;

/// Extension trait for `AsyncWrite`.
pub trait AsyncWriteLimitExt: AsyncWrite {
    /// Limit the write rate of the underlying writer.
    ///
    /// # Arguments
    ///
    /// * `burst` - Maximum burst size in bits.
    /// * `rate` - Maximum write rate in bits per second.
    fn limit_rate(self, burst: usize, rate: usize) -> Rate<Self>
    where
        Self: Sized,
    {
        Rate::new(self, burst, rate)
    }
}

impl<T> AsyncWriteLimitExt for T where T: AsyncWrite {}

/// Extension trait for `AsyncRead`.
pub trait AsyncReadDelayExt: AsyncRead {
    /// Delays incoming data by the given amount of milliseconds.
    ///
    /// Returns a future which must be polled continuously. See [`Delay`] for
    /// more details.
    ///
    /// # Arguments
    ///
    /// * `delay` - Delay in milliseconds.
    fn delay(self, delay: usize) -> (Delay<Self>, DelayFuture<Self>)
    where
        Self: Sized,
    {
        Delay::new(self, delay)
    }
}

impl<T> AsyncReadDelayExt for T where T: AsyncRead {}
