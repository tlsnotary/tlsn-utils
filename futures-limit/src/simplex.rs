use std::{
    collections::VecDeque,
    future::Future,
    io::Result,
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};

use bytes::{Buf, BytesMut};
use futures::{
    io::{ReadHalf, WriteHalf},
    AsyncRead, AsyncReadExt, AsyncWrite,
};
use futures_timer::Delay;
use pin_project_lite::pin_project;

use crate::Instant;

/// Returns a simplex connection pair.
///
/// # Arguments
///
/// * `params` - Parameters for the connection.
pub fn simplex(params: Params) -> (ReadHalf<Simplex>, WriteHalf<Simplex>) {
    Simplex::new(params).split()
}

#[derive(Debug, Clone, Copy)]
struct Packet {
    len: usize,
    /// Time when the packet is ready.
    ready: Instant,
}

/// Unidirectional pipe with configurable bandwidth, latency and buffer size.
///
/// Implementation is based on the simplex in `tokio`.
#[derive(Debug)]
pub struct Simplex {
    params: Params,
    buf: BytesMut,
    /// Packets in the buffer.
    packets: VecDeque<Packet>,
    /// Whether the write side has closed.
    is_closed: bool,
    /// Waker for the read side.
    read_waker: Option<Waker>,
    /// Read bucket.
    read_bucket: TokenBucket,
    /// Timer to wake up the read side when the latency has elapsed.
    read_timer: Pin<Box<Delay>>,
    /// Waker for the write side.
    write_waker: Option<Waker>,
    /// Write bucket.
    write_bucket: TokenBucket,
}

impl Simplex {
    /// Create a new `Simplex`.
    ///
    /// # Panics
    ///
    /// Panics if `tx_rate` or `rx_rate` are less than 8 bits per second.
    ///
    /// # Arguments
    ///
    /// * `params` - Parameters for the connection.
    pub fn new(params: Params) -> Self {
        assert!(
            params.tx_rate >= 8,
            "tx_rate must be at least 8 bits per second"
        );
        assert!(
            params.rx_rate >= 8,
            "rx_rate must be at least 8 bits per second"
        );

        let tx_bytes_per_sec = params.tx_rate >> 3;
        let rx_bytes_per_sec = params.rx_rate >> 3;

        let write_bucket = TokenBucket::new(DEFAULT_BUCKET_CAPACITY, tx_bytes_per_sec >> 20);
        let read_bucket = TokenBucket::new(DEFAULT_BUCKET_CAPACITY, rx_bytes_per_sec >> 20);

        Self {
            params,
            buf: BytesMut::new(),
            packets: VecDeque::new(),
            is_closed: false,
            read_waker: None,
            read_bucket,
            read_timer: Box::pin(Delay::new(Duration::from_millis(0))),
            write_waker: None,
            write_bucket,
        }
    }

    fn close_write(&mut self) {
        self.is_closed = true;
        // needs to notify any readers that no more data will come
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
    }

    fn poll_write_internal(&mut self, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }

        let len = self.params.buf_size - self.buf.len();
        if len == 0 {
            // Buffer is full, so we need to wait for some data to be read.
            self.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        self.write_bucket.refill();
        let len = len.min(self.write_bucket.available());
        if len == 0 {
            // No tokens available, so we need to wait for the bucket to refill.
            assert!(self.write_bucket.poll_refill(cx).is_pending());
            return Poll::Pending;
        }

        let len = len.min(buf.len());
        self.buf.extend_from_slice(&buf[..len]);
        self.write_bucket.consume(len);
        self.packets.push_front(Packet {
            len,
            ready: Instant::now() + Duration::from_millis(self.params.latency as u64),
        });

        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }

        Poll::Ready(Ok(len))
    }

    fn poll_read_internal(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize>> {
        if self.buf.has_remaining() {
            self.read_bucket.refill();
            if self.read_bucket.is_empty() {
                // No tokens available, so we need to wait for the bucket to refill.
                assert!(self.read_bucket.poll_refill(cx).is_pending());
                return Poll::Pending;
            }

            // Maximum amount of bytes that can be processed this poll.
            let max_len = self
                .buf
                .remaining()
                .min(buf.len())
                .min(self.read_bucket.available());

            // Read packets in reverse order to process the oldest packets first.
            //
            // Steps:
            //   1. Check that the packet is ready to be read (latency has elapsed). If not,
            //      register a waker for when it is ready.
            //   2. Read as many bytes as possible from the packet. Update the packet length
            //      if it is partially read.
            //   3. Remove fully read packets from the queue.
            let mut remaining = max_len;
            let mut complete = 0;
            let now = Instant::now();
            for Packet {
                len: packet_len,
                ready,
            } in self.packets.iter_mut().rev()
            {
                let time_left = ready.saturating_duration_since(now);
                if time_left.as_millis() > 0 {
                    self.read_timer.reset(time_left);
                    // Poll timer to register waker.
                    assert!(self.read_timer.as_mut().poll(cx).is_pending());
                    break;
                }

                let len = (*packet_len).min(remaining);
                if len == *packet_len {
                    complete += 1;
                } else {
                    // Partial read, update packet length.
                    *packet_len -= len;
                }

                remaining -= len;
                if remaining == 0 {
                    break;
                }
            }

            if remaining == max_len {
                // No packets are ready to be read, so we need to wait for the timer to expire.
                return Poll::Pending;
            }

            // Remove packets that have been fully read.
            self.packets.truncate(self.packets.len() - complete);

            let len = max_len - remaining;
            buf[..len].copy_from_slice(&self.buf[..len]);
            self.buf.advance(len);
            self.read_bucket.consume(len);
            if len > 0 {
                // The passed `buf` might have been empty, don't wake up if
                // no bytes have been moved.
                if let Some(waker) = self.write_waker.take() {
                    waker.wake();
                }
            }

            Poll::Ready(Ok(len))
        } else if self.is_closed {
            Poll::Ready(Ok(0))
        } else {
            self.read_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl AsyncWrite for Simplex {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize>> {
        self.poll_write_internal(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<()>> {
        self.close_write();
        Poll::Ready(Ok(()))
    }
}

impl AsyncRead for Simplex {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        self.poll_read_internal(cx, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::{poll, AsyncReadExt, AsyncWriteExt};
    use mock_instant::thread_local::MockClock;
    use pollster::FutureExt;

    #[test]
    fn test_simplex() {
        async {
            let mut io = Simplex::new(Params {
                buf_size: 1024,
                tx_rate: 1024 * 8,
                rx_rate: 1024 * 8,
                latency: 0,
            });

            let data = b"hello world";
            io.write_all(data).await.unwrap();

            let mut buf = [0; 1024];
            let len = io.read(&mut buf).await.unwrap();

            assert_eq!(len, data.len());
            assert_eq!(&buf[..len], data);
        }
        .block_on()
    }

    #[test]
    fn test_simplex_burst_write() {
        async {
            let mut io = Simplex::new(Params {
                buf_size: DEFAULT_BUCKET_CAPACITY,
                tx_rate: 8,
                rx_rate: 1024 * 8,
                latency: 0,
            });

            let data = vec![0; DEFAULT_BUCKET_CAPACITY];

            // Burst write should accept the full buffer.
            assert_eq!(
                poll!(io.write(&data)).map(|r| r.unwrap()),
                Poll::Ready(data.len())
            );
            assert_eq!(io.write_bucket.tokens, 0);
        }
        .block_on()
    }

    #[test]
    fn test_simplex_latency() {
        async {
            let mut io = Simplex::new(Params {
                buf_size: DEFAULT_BUCKET_CAPACITY,
                tx_rate: 8,
                rx_rate: 1024 * 8,
                latency: 2,
            });

            let mut data = vec![0; DEFAULT_BUCKET_CAPACITY];
            io.write_all(&data).await.unwrap();

            // No time has elapsed, so no data should be available.
            assert_eq!(poll!(io.read(&mut data)).map(|r| r.unwrap()), Poll::Pending);

            // Latency still hasn't elapsed.
            MockClock::advance(Duration::from_millis(1));
            assert_eq!(poll!(io.read(&mut data)).map(|r| r.unwrap()), Poll::Pending);

            // Latency has elapsed, so data should be available.
            MockClock::advance(Duration::from_millis(1));
            assert_eq!(
                poll!(io.read(&mut data)).map(|r| r.unwrap()),
                Poll::Ready(DEFAULT_BUCKET_CAPACITY)
            );
        }
        .block_on()
    }
}
