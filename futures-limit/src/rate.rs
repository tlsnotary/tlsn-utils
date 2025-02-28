use std::{
    io::{IoSliceMut, Result},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{AsyncRead, AsyncWrite};
use pin_project_lite::pin_project;

use crate::bucket::TokenBucket;

const M: u64 = 1_000_000;

pin_project! {
    /// Rate limiting wrapper for `AsyncWrite`.
    #[derive(Debug)]
    pub struct Rate<Io> {
        #[pin] io: Io,
        bucket: TokenBucket,
    }
}

impl<Io> Rate<Io> {
    /// Create a new rate limiter.
    ///
    /// # Arguments
    ///
    /// * `io` - Underlying I/O object.
    /// * `burst` - Maximum burst size in bits.
    /// * `rate` - Maximum write rate in bits per second.
    pub fn new(io: Io, burst: usize, rate: usize) -> Self {
        // Bucketing is done with microsecond granularity.
        // Each token represents one-millionth of a byte.
        let tokens = ((burst as u64) * M).div_ceil(8);
        let tokens_per_micro_sec = (rate as u64).div_ceil(8);

        let bucket = TokenBucket::new(tokens, tokens_per_micro_sec);

        Self { io, bucket }
    }
}

impl<Io> Rate<Io>
where
    Io: AsyncWrite,
{
    fn poll_write_internal(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        let this = self.project();

        this.bucket.refill();

        let available = (this.bucket.available() / M) as usize;
        if available == 0 {
            this.bucket.poll_refill(cx);
            return Poll::Pending;
        }

        let len = buf.len().min(available);

        let res = this.io.poll_write(cx, &buf[..len]);

        if let Poll::Ready(Ok(n)) = &res {
            this.bucket.consume((*n as u64) * M);
        }

        res
    }
}

impl<Io> AsyncWrite for Rate<Io>
where
    Io: AsyncWrite,
{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        self.poll_write_internal(cx, buf)
    }

    #[inline]
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.project().io.poll_flush(cx)
    }

    #[inline]
    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.project().io.poll_close(cx)
    }
}

impl<Io> AsyncRead for Rate<Io>
where
    Io: AsyncRead,
{
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.project().io.poll_read(cx, buf)
    }

    #[inline]
    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<Result<usize>> {
        self.project().io.poll_read_vectored(cx, bufs)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use futures::{AsyncWriteExt, io::sink, poll};
    use mock_instant::thread_local::MockClock;

    // Tests that the burst size is respected.
    #[pollster::test]
    async fn test_rate_burst() {
        let data = b"hello world";

        let mut io = Rate::new(sink(), (data.len() - 1) * 8, 0);

        let n = io.write(data).await.unwrap();

        assert_eq!(n, data.len() - 1);
    }

    // Tests that the burst will allow all data to be written when it is less than
    // the burst size.
    #[pollster::test]
    async fn test_rate_burst_all() {
        let data = b"hello world";

        let mut io = Rate::new(sink(), data.len() * 8, 0);

        let n = io.write(data).await.unwrap();

        assert_eq!(n, data.len());
    }

    #[pollster::test]
    async fn test_rate_limit() {
        let data = b"hello world";

        let mut io = Rate::new(sink(), data.len() * 8, 8);

        let n = io.write(data).await.unwrap();

        assert_eq!(n, data.len());

        let mut write = io.write(data);

        assert!(poll!(&mut write).is_pending());

        MockClock::advance(Duration::from_secs(1));

        let Poll::Ready(Ok(n)) = poll!(write) else {
            panic!("poll should be ready");
        };

        // 1 byte per second.
        assert_eq!(n, 1);

        let mut write = io.write(data);

        assert!(poll!(&mut write).is_pending());
    }
}
