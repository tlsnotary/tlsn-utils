use std::{
    collections::VecDeque,
    io::Result,
    pin::Pin,
    task::{Context, Poll, Waker, ready},
    time::Duration,
};

use bytes::{Buf, BytesMut};
use futures::{AsyncRead, AsyncWrite, lock::BiLock};
use futures_timer::Delay as DelayTimer;
use pin_project_lite::pin_project;

use crate::Instant;

const BUF_SIZE: usize = 16 * 1024; // 16 KiB

/// Delay wrapper for `AsyncRead`.
///
/// This wrapper will delay incoming data by the provided amount of
/// milliseconds. A corresponding future is also returned. This future should be
/// spawned onto a dedicated thread to ensure that the delay is accurate.
///
/// # Warning
///
/// Incoming data is continuously read from the underlying I/O object. This
/// buffer will continue to grow unbounded if the data is processed slower than
/// it is received.
#[derive(Debug)]
pub struct Delay<Io> {
    read: BiLock<Simplex>,
    write: BiLock<Io>,
}

impl<Io> Delay<Io> {
    /// Create a new delay.
    ///
    /// Returns a future which must be polled continuously. This future should
    /// be spawned onto a dedicated thread to ensure that the delay is accurate.
    ///
    /// # Arguments
    ///
    /// * `io` - Underlying I/O object.
    /// * `delay` - Delay in milliseconds.
    pub fn new(io: Io, delay: usize) -> (Self, DelayFuture<Io>) {
        let simplex = Simplex::new(delay);

        let (delay_read, delay_write) = BiLock::new(simplex);
        let (io_read, io_write) = BiLock::new(io);

        (
            Self {
                read: delay_read,
                write: io_write,
            },
            DelayFuture {
                delay: delay as u64,
                read: io_read,
                buf: vec![0; BUF_SIZE].into_boxed_slice(),
                write: delay_write,
            },
        )
    }
}

impl<Io> AsyncRead for Delay<Io>
where
    Io: AsyncRead,
{
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        let mut read = ready!(self.read.poll_lock(cx));
        read.poll_read(cx, buf)
    }
}

impl<Io> AsyncWrite for Delay<Io>
where
    Io: AsyncWrite,
{
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize>> {
        let mut write = ready!(self.write.poll_lock(cx));
        write.as_pin_mut().poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut write = ready!(self.write.poll_lock(cx));
        write.as_pin_mut().poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<()>> {
        let mut write = ready!(self.write.poll_lock(cx));
        write.as_pin_mut().poll_close(cx)
    }
}

pin_project! {
    /// Future returned by [`Delay::new`].
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct DelayFuture<Io> {
        delay: u64,
        read: BiLock<Io>,
        buf: Box<[u8]>,
        write: BiLock<Simplex>,
    }
}

impl<Io> Future for DelayFuture<Io>
where
    Io: AsyncRead,
{
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();

        let mut write = ready!(this.write.poll_lock(cx));
        let mut read = ready!(this.read.poll_lock(cx));

        let mut len = 0;
        let mut closed = false;
        while let Poll::Ready(res) = read.as_pin_mut().poll_read(cx, this.buf) {
            match res {
                Ok(n) => {
                    if n == 0 {
                        closed = true;
                        break;
                    }
                    len += n;
                    write.buf.extend_from_slice(&this.buf[..n]);
                }
                Err(err) => {
                    write.close_write();
                    return Poll::Ready(Err(err));
                }
            }
        }

        if len > 0 {
            write.packets.push_front(Packet {
                len,
                ready: Instant::now() + Duration::from_millis(*this.delay),
            });
            write.wake_reader();
        }

        if closed {
            write.close_write();
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Packet {
    len: usize,
    /// Time when the packet is ready.
    ready: Instant,
}

#[derive(Debug)]
struct Simplex {
    buf: BytesMut,
    /// Packets in the buffer.
    packets: VecDeque<Packet>,
    /// Whether the write side has closed.
    is_closed: bool,
    /// Waker for the read side.
    read_waker: Option<Waker>,
    /// Timer to wake up the read side when the latency has elapsed.
    read_timer: Pin<Box<DelayTimer>>,
}

impl Simplex {
    fn new(delay: usize) -> Self {
        Self {
            buf: BytesMut::with_capacity(16 * 1024),
            packets: VecDeque::new(),
            is_closed: false,
            read_waker: None,
            read_timer: Box::pin(DelayTimer::new(Duration::from_millis(delay as u64))),
        }
    }

    fn close_write(&mut self) {
        self.is_closed = true;
        // needs to notify any readers that no more data will come
        self.wake_reader();
    }

    fn wake_reader(&mut self) {
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
    }

    fn poll_read(&mut self, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<Result<usize>> {
        if self.buf.has_remaining() {
            // Maximum amount of bytes that can be processed this poll.
            let max_len = self.buf.remaining().min(buf.len());

            // Read packets in reverse order to process the oldest packets first.
            //
            // Steps:
            //   1. Check that the packet is ready to be read (latency has elapsed). If not,
            //      register a waker for when it is ready.
            //   2. Read as many bytes as possible from the packet. Update the packet length
            //      if it is partially read.
            //   3. Remove fully read packets from the queue.
            let mut remaining = max_len;
            let mut done_packets = 0;
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
                    done_packets += 1;
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
            self.packets.truncate(self.packets.len() - done_packets);

            let len = max_len - remaining;
            buf[..len].copy_from_slice(&self.buf[..len]);
            self.buf.advance(len);

            Poll::Ready(Ok(len))
        } else if self.is_closed {
            Poll::Ready(Ok(0))
        } else {
            self.read_waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{pin::pin, time::Duration};

    use super::*;
    use futures::{AsyncWriteExt, future::poll_fn, poll};
    use futures_plex::simplex;
    use mock_instant::thread_local::MockClock;

    #[pollster::test]
    async fn test_delay() {
        let data = b"hello world";
        const DELAY: usize = 1;

        let (read, mut write) = simplex(100);
        let (mut delay, mut fut) = Delay::new(read, DELAY);

        write.write_all(data).await.unwrap();
        write.flush().await.unwrap();

        assert!(poll!(&mut fut).is_pending());

        let mut buf = vec![0u8; 11];
        let res = poll!(poll_fn(|cx| pin!(&mut delay).poll_read(cx, &mut buf)));

        // Data should not be available yet.
        assert!(res.is_pending());

        MockClock::advance(Duration::from_millis(DELAY as u64));

        let res = poll!(poll_fn(|cx| pin!(&mut delay).poll_read(cx, &mut buf)));

        // Data should be available now.
        assert!(matches!(res, Poll::Ready(Ok(11))));

        write.close().await.unwrap();

        assert!(poll!(&mut fut).is_ready());
    }
}
