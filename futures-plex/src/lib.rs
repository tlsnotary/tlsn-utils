#![doc = include_str!("../README.md")]

use std::{
    io::{Read, Write},
    pin::{Pin, pin},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{self, Poll, Waker, ready},
};

use futures::{AsyncBufRead, AsyncRead, AsyncWrite, future::poll_fn, io, task::AtomicWaker};

mod half;

pub use half::{ReadGuard, ReadHalf, WriteGuard, WriteHalf};

/// State that must be reachable without acquiring the `BiLock` guarding a
/// [`SimplexStream`].
///
/// Dropping a [`DuplexStream`] has to close both directions, but the peer may
/// be holding the corresponding `BiLock` (it can even be the code that drops
/// us), so blocking on it would deadlock. Only what a close needs lives here:
/// the closed flag, and a waker slot per parked side so that a close can wake
/// the peer without the lock.
///
/// The wakers used by the data path stay in the [`SimplexStream`], under the
/// lock, so that `advance`/`advance_mut` don't touch an atomic.
#[derive(Debug)]
struct CloseState {
    /// Set once this side is closed: readers then observe EOF and writers
    /// observe `BrokenPipe`.
    ///
    /// A `SimplexStream` only carries one direction, so both closers set the
    /// same flag.
    closed: AtomicBool,
    /// Waker of a task parked in [`SimplexStream::poll_get`], woken when this
    /// stream is closed.
    ///
    /// It is not cleared when the task is woken for data instead, so a close
    /// may spuriously wake a task that is no longer parked.
    read_closer: AtomicWaker,
    /// Waker of a task parked in [`SimplexStream::poll_mut`], woken when this
    /// stream is closed.
    write_closer: AtomicWaker,
}

impl CloseState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            closed: AtomicBool::new(false),
            read_closer: AtomicWaker::new(),
            write_closer: AtomicWaker::new(),
        })
    }

    /// Closes the write side and returns the reader's waker, if one is parked.
    ///
    /// The flag is published before the waker is taken: pollers register their
    /// waker and then re-check the flag, so a close racing a registration
    /// cannot lose the wakeup.
    fn close_write(&self) -> Option<Waker> {
        self.closed.store(true, Ordering::Release);
        self.read_closer.take()
    }

    /// Closes the read side and returns the writer's waker, if one is parked.
    fn close_read(&self) -> Option<Waker> {
        self.closed.store(true, Ordering::Release);
        self.write_closer.take()
    }
}

/// A bidirectional pipe to read and write bytes in memory.
///
/// A pair of `DuplexStream`s are created together, and they act as a "channel"
/// that can be used as in-memory IO types. Writing to one of the pairs will
/// allow that data to be read from the other, and vice versa.
///
/// # Closing a `DuplexStream`
///
/// If one end of the `DuplexStream` channel is dropped, any pending reads on
/// the other side will continue to read data until the buffer is drained, then
/// they will signal EOF by returning 0 bytes. Any writes to the other side,
/// including pending ones (that are waiting for free space in the buffer) will
/// return `Err(BrokenPipe)` immediately.
///
/// # Example
///
/// ```
/// # async fn ex() -> std::io::Result<()> {
/// # use futures::{AsyncReadExt, AsyncWriteExt};
/// let (mut client, mut server) = futures_plex::duplex(64);
///
/// client.write_all(b"ping").await?;
///
/// let mut buf = [0u8; 4];
/// server.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"ping");
///
/// server.write_all(b"pong").await?;
///
/// client.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"pong");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct DuplexStream {
    read: ReadHalf<SimplexStream>,
    write: WriteHalf<SimplexStream>,
    // Close state of the stream behind `read`, whose read side is closed when
    // this duplex is dropped. Kept outside the `BiLock` so `Drop` never blocks.
    read_close: Arc<CloseState>,
    // Close state of the stream behind `write`, whose write side is closed
    // when this duplex is dropped.
    write_close: Arc<CloseState>,
}

impl DuplexStream {
    /// Read data from this duplex into the provided writer.
    pub fn poll_read_to<W: AsyncWrite + Unpin>(
        &self,
        cx: &mut task::Context<'_>,
        wr: W,
    ) -> Poll<io::Result<usize>> {
        ready!(self.poll_lock_read(cx)).poll_read_to(cx, wr)
    }

    /// Write data from the provided reader into this duplex.
    pub fn poll_write_from<R: AsyncRead + Unpin>(
        &self,
        cx: &mut task::Context<'_>,
        rd: R,
    ) -> Poll<io::Result<usize>> {
        ready!(self.poll_lock_write(cx)).poll_write_from(cx, rd)
    }

    /// Attempt to acquire a lock on the read side, returning `Poll::Pending`
    /// if it can't be acquired.
    pub fn poll_lock_read(&self, cx: &mut task::Context<'_>) -> Poll<ReadGuard<'_, SimplexStream>> {
        self.read.poll_lock(cx)
    }

    /// Attempt to acquire a lock on the write side, returning `Poll::Pending`
    /// if it can't be acquired.
    pub fn poll_lock_write(
        &self,
        cx: &mut task::Context<'_>,
    ) -> Poll<WriteGuard<'_, SimplexStream>> {
        self.write.poll_lock(cx)
    }
}

// ===== impl DuplexStream =====

/// Create a new pair of `DuplexStream`s that act like a pair of connected
/// sockets.
///
/// The `max_buf_size` argument is the maximum amount of bytes that can be
/// written to a side before the write returns `Poll::Pending`.
///
/// # Panics
///
/// This function panics if `max_buf_size` is 0: with no space to ever write,
/// both sides would wait forever.
#[track_caller]
pub fn duplex(max_buf_size: usize) -> (DuplexStream, DuplexStream) {
    assert!(max_buf_size > 0, "`max_buf_size` must be greater than 0");

    let stream_0 = SimplexStream::new_unsplit(max_buf_size);
    let stream_1 = SimplexStream::new_unsplit(max_buf_size);
    let (close_0, close_1) = (stream_0.close.clone(), stream_1.close.clone());

    let (read_0, write_0) = half::split(stream_0);
    let (read_1, write_1) = half::split(stream_1);

    (
        DuplexStream {
            read: read_0,
            write: write_1,
            read_close: close_0.clone(),
            write_close: close_1.clone(),
        },
        DuplexStream {
            read: read_1,
            write: write_0,
            read_close: close_1,
            write_close: close_0,
        },
    )
}

impl Drop for DuplexStream {
    fn drop(&mut self) {
        // Close both directions without taking the `BiLock`s: the peer may be
        // holding one (it may even be the code that is dropping us), so
        // blocking here would deadlock. Only atomics are touched, and the
        // wakers are woken after the flags are published.
        let writer_waker = self.read_close.close_read();
        let reader_waker = self.write_close.close_write();

        if let Some(waker) = writer_waker {
            waker.wake();
        }
        if let Some(waker) = reader_waker {
            waker.wake();
        }
    }
}

impl AsyncRead for DuplexStream {
    // Previous rustc required this `self` to be `mut`, even though newer
    // versions recognize it isn't needed to call `lock()`. So for
    // compatibility, we include the `mut` and `allow` the lint.
    //
    // See https://github.com/rust-lang/rust/issues/73592
    #[allow(unused_mut)]
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.read).poll_read(cx, buf)
    }
}

impl AsyncWrite for DuplexStream {
    #[allow(unused_mut)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.write).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.write).poll_write_vectored(cx, bufs)
    }

    #[allow(unused_mut)]
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }

    #[allow(unused_mut)]
    fn poll_close(mut self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.write).poll_close(cx)
    }
}

impl Read for DuplexStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let fut = poll_fn(|cx| AsyncRead::poll_read(Pin::new(self), cx, buf));
        futures::executor::block_on(fut)
    }
}

impl Write for DuplexStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let fut = poll_fn(|cx| AsyncWrite::poll_write(Pin::new(self), cx, buf));
        futures::executor::block_on(fut)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A unidirectional pipe to read and write bytes in memory.
///
/// It can be constructed by [`simplex`] function which will create a pair of
/// reader and writer or by calling [`SimplexStream::new_unsplit`] that will
/// create a handle for both reading and writing.
///
/// # Example
///
/// ```
/// # async fn ex() -> std::io::Result<()> {
/// # use futures::{AsyncReadExt, AsyncWriteExt};
/// let (mut receiver, mut sender) = futures_plex::simplex(64);
///
/// sender.write_all(b"ping").await?;
///
/// let mut buf = [0u8; 4];
/// receiver.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"ping");
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SimplexStream {
    // The buffer storing the bytes written, also read from.
    buffer: Box<[u8]>,
    // Pointer to the next byte in the buffer.
    ptr: usize,
    // Number of bytes in the buffer.
    len: usize,
    // Closed flag and close-notification wakers. Shared with the owning
    // `DuplexStream` (if any) so that it can close this stream without taking
    // the lock.
    close: Arc<CloseState>,
    // If the `read` side has been polled and is pending, this is the waker
    // for that parked task.
    read_waker: Option<Waker>,
    // If the `write` side has filled the `max_buf_size` and returned
    // `Poll::Pending`, this is the waker for that parked task.
    write_waker: Option<Waker>,
}

// ===== impl SimplexStream =====

/// Creates unidirectional buffer that acts like in memory pipe.
///
/// The `max_buf_size` argument is the maximum amount of bytes that can be
/// written to a buffer before the it returns `Poll::Pending`.
///
/// # Reunite reader and writer
///
/// The reader and writer half can be unified into a single structure
/// of `SimplexStream` that supports both reading and writing or
/// the `SimplexStream` can be already created as unified structure
/// using [`SimplexStream::new_unsplit()`].
///
/// ```
/// # async fn ex() -> std::io::Result<()> {
/// # use futures::{AsyncReadExt, AsyncWriteExt};
/// let (reader, writer) = futures_plex::simplex(64);
/// let mut simplex_stream = reader.reunite(writer).unwrap();
/// simplex_stream.write_all(b"hello").await?;
///
/// let mut buf = [0u8; 5];
/// simplex_stream.read_exact(&mut buf).await?;
/// assert_eq!(&buf, b"hello");
/// # Ok(())
/// # }
/// ```
pub fn simplex(max_buf_size: usize) -> (ReadHalf<SimplexStream>, WriteHalf<SimplexStream>) {
    half::split(SimplexStream::new_unsplit(max_buf_size))
}

impl SimplexStream {
    /// Creates unidirectional buffer that acts like in memory pipe. To create
    /// split version with separate reader and writer you can use
    /// [`simplex`] function.
    ///
    /// The `buf_size` argument is the maximum amount of bytes that can be
    /// written to a buffer before the it returns `Poll::Pending`.
    pub fn new_unsplit(buf_size: usize) -> SimplexStream {
        Self::with_close(buf_size, CloseState::new())
    }

    fn with_close(buf_size: usize, close: Arc<CloseState>) -> SimplexStream {
        SimplexStream {
            buffer: vec![0; buf_size].into_boxed_slice(),
            ptr: 0,
            len: 0,
            close,
            read_waker: None,
            write_waker: None,
        }
    }

    /// Returns the number of bytes that can be read from this buffer.
    pub fn remaining(&self) -> usize {
        self.len
    }

    /// Returns the number of bytes that can be written into this buffer.
    pub fn remaining_mut(&self) -> usize {
        self.buffer.len() - self.len
    }

    /// Returns a reference to the first contiguous chunk of data in the buffer.
    ///
    /// For a ring buffer, data may wrap around. This returns only the first
    /// contiguous chunk. Call again after `advance()` to get remaining data.
    pub fn get(&self) -> &[u8] {
        if self.len == 0 {
            return &[];
        }
        let cap = self.buffer.len();
        let end = self.ptr + self.len;
        if end <= cap {
            &self.buffer[self.ptr..end]
        } else {
            // Data wraps, return first contiguous chunk
            &self.buffer[self.ptr..cap]
        }
    }

    /// Returns a reference to the data in the buffer when data is available.
    pub fn poll_get(&mut self, cx: &mut task::Context<'_>) -> Poll<io::Result<&[u8]>> {
        if self.remaining() > 0 {
            Poll::Ready(Ok(self.get()))
        } else if self.close.closed.load(Ordering::Acquire) {
            Poll::Ready(Ok(&[]))
        } else {
            // Park on the in-lock waker for data, and on the lock-free one so
            // that a concurrent close can reach us. Then re-check the flag: a
            // close may have happened before the registration.
            self.read_waker = Some(cx.waker().clone());
            self.close.read_closer.register(cx.waker());
            if self.close.closed.load(Ordering::Acquire) {
                Poll::Ready(Ok(&[]))
            } else {
                Poll::Pending
            }
        }
    }

    /// Returns a mutable slice to the first contiguous chunk of available
    /// capacity in the buffer.
    ///
    /// For a ring buffer, available space may wrap around. This returns only
    /// the first contiguous chunk. Call again after `advance_mut()` to get
    /// remaining space.
    pub fn get_mut(&mut self) -> &mut [u8] {
        let cap = self.buffer.len();
        let avail = cap - self.len;
        if avail == 0 {
            return &mut [];
        }
        let tail = (self.ptr + self.len) % cap;
        if tail < self.ptr {
            // Tail wrapped around, contiguous space is tail..ptr
            &mut self.buffer[tail..self.ptr]
        } else {
            // Tail is at or after ptr, contiguous space is tail..cap
            &mut self.buffer[tail..cap]
        }
    }

    /// Returns a mutable reference to the available space in the buffer when
    /// there is space available.
    pub fn poll_mut(&mut self, cx: &mut task::Context<'_>) -> Poll<io::Result<&mut [u8]>> {
        if self.is_closed() {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.remaining_mut();
        if avail == 0 {
            // Park on the in-lock waker for space, and on the lock-free one so
            // that a concurrent close can reach us. Then re-check the flag.
            self.write_waker = Some(cx.waker().clone());
            self.close.write_closer.register(cx.waker());
            if self.is_closed() {
                return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
            }
            Poll::Pending
        } else {
            Poll::Ready(Ok(self.get_mut()))
        }
    }

    /// Returns `true` if this stream has been closed, meaning that no more
    /// bytes can be written into it.
    fn is_closed(&self) -> bool {
        self.close.closed.load(Ordering::Acquire)
    }

    /// Advances the read cursor.
    ///
    /// This method should be called after reading from the simplex using
    /// [`get`](Self::get) or [`poll_get`](Self::poll_get) to advance the read
    /// cursor.
    ///
    /// # Panics
    ///
    /// Panics if the provided amount exceeds the data in the buffer.
    ///
    /// # Arguments
    ///
    /// * `amt` - The number of bytes read.
    pub fn advance(&mut self, amt: usize) {
        if amt == 0 {
            return;
        }

        assert!(amt <= self.len, "out of bounds");
        self.ptr = (self.ptr + amt) % self.buffer.len();
        self.len -= amt;

        // Wake the writer side now that space is available.
        if let Some(waker) = self.write_waker.take() {
            waker.wake();
        }
    }

    /// Advances the write cursor.
    ///
    /// This method should be called after writing to the simplex using
    /// [`get_mut`](Self::get_mut) or [`poll_mut`](Self::poll_mut) to advance
    /// the write cursor.
    ///
    /// # Panics
    ///
    /// Panics if the provided amount exceeds the spare capacity.
    ///
    /// # Arguments
    ///
    /// * `amt` - The number of bytes written.
    pub fn advance_mut(&mut self, amt: usize) {
        if amt == 0 {
            return;
        }

        assert!(self.len + amt <= self.buffer.len(), "out of bounds");
        self.len += amt;

        // Wake the read side now that data is available.
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
    }

    /// Read data from this simplex into the provided writer.
    pub fn poll_read_to<W: AsyncWrite + Unpin>(
        &mut self,
        cx: &mut task::Context<'_>,
        wr: W,
    ) -> Poll<io::Result<usize>> {
        let buf = ready!(self.poll_get(cx))?;
        let len = ready!(pin!(wr).poll_write(cx, buf))?;
        self.advance(len);
        Poll::Ready(Ok(len))
    }

    /// Write data from the provided reader into this simplex.
    pub fn poll_write_from<R: AsyncRead + Unpin>(
        &mut self,
        cx: &mut task::Context<'_>,
        rd: R,
    ) -> Poll<io::Result<usize>> {
        let buf = ready!(self.poll_mut(cx))?;
        let len = ready!(pin!(rd).poll_read(cx, buf))?;
        self.advance_mut(len);
        Poll::Ready(Ok(len))
    }

    /// Closes the write side and returns the reader's waker, if one is parked.
    pub(crate) fn close_write(&self) -> Option<Waker> {
        self.close.close_write()
    }

    fn poll_read_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let src = ready!(self.poll_get(cx))?;

        if src.is_empty() {
            return Poll::Ready(Ok(0));
        }

        let len = buf.len().min(src.len());
        buf[..len].copy_from_slice(&src[..len]);
        self.advance(len);

        Poll::Ready(Ok(len))
    }

    fn poll_write_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let dest = ready!(self.poll_mut(cx))?;
        debug_assert!(
            !dest.is_empty(),
            "returned ready when no space is available"
        );

        let len = buf.len().min(dest.len());
        dest[..len].copy_from_slice(&buf[..len]);
        self.advance_mut(len);

        Poll::Ready(Ok(len))
    }

    fn poll_write_vectored_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        let mut dest = ready!(self.poll_mut(cx))?;
        debug_assert!(
            !dest.is_empty(),
            "returned ready when no space is available"
        );

        let avail = dest.len();
        let mut amt = 0;
        for buf in bufs {
            if amt >= avail {
                break;
            }

            let len = buf.len().min(dest.len());
            dest[..len].copy_from_slice(&buf[..len]);

            dest = &mut dest[len..];
            amt += len;
        }

        self.advance_mut(amt);

        Poll::Ready(Ok(amt))
    }
}

impl AsyncRead for SimplexStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_read_internal(cx, buf)
    }
}

impl AsyncBufRead for SimplexStream {
    fn poll_fill_buf(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<io::Result<&[u8]>> {
        Pin::get_mut(self).poll_get(cx)
    }

    fn consume(mut self: Pin<&mut Self>, amt: usize) {
        self.advance(amt);
    }
}

impl AsyncWrite for SimplexStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_internal(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        self.poll_write_vectored_internal(cx, bufs)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        if let Some(waker) = self.close_write() {
            waker.wake();
        }
        Poll::Ready(Ok(()))
    }
}

impl Read for SimplexStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let fut = poll_fn(|cx| AsyncRead::poll_read(Pin::new(self), cx, buf));
        futures::executor::block_on(fut)
    }
}

impl Write for SimplexStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let fut = poll_fn(|cx| AsyncWrite::poll_write(Pin::new(self), cx, buf));
        futures::executor::block_on(fut)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
