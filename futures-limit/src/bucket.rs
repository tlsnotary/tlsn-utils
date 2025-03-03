use std::{future::Future, pin::Pin, task::Context, time::Duration};

use futures_timer::Delay;

use crate::Instant;

/// Default interval in millis in which the write side is woken up when
/// reaching throughput limits. This sets the granularity of the rate limiting
/// and an upper bound on the throughput.
const WAKE_INTERVAL: u64 = 1;

#[derive(Debug)]
pub(crate) struct TokenBucket {
    capacity: u64,
    tokens: u64,
    /// Refill rate in tokens per micro second.
    rate: u64,
    last_refill: Instant,
    timer: Pin<Box<Delay>>,
}

impl TokenBucket {
    /// Create a new `TokenBucket`.
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of tokens the bucket can hold.
    /// * `rate` - Refill rate in tokens per microsecond.
    pub(crate) fn new(capacity: u64, rate: u64) -> Self {
        Self {
            capacity,
            tokens: capacity,
            rate,
            last_refill: Instant::now(),
            timer: Box::pin(Delay::new(Duration::from_millis(WAKE_INTERVAL))),
        }
    }

    pub(crate) fn available(&self) -> u64 {
        self.tokens
    }

    pub(crate) fn consume(&mut self, amount: u64) {
        self.tokens = self.tokens.saturating_sub(amount);
    }

    pub(crate) fn poll_refill(&mut self, cx: &mut Context<'_>) {
        self.timer.reset(Duration::from_millis(WAKE_INTERVAL));
        assert!(self.timer.as_mut().poll(cx).is_pending());
    }

    pub(crate) fn refill(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_micros() as u64;
        if elapsed == 0 {
            return;
        }

        let tokens = elapsed.saturating_mul(self.rate);
        self.tokens = self.tokens.saturating_add(tokens).min(self.capacity);
        self.last_refill = now;
    }
}
