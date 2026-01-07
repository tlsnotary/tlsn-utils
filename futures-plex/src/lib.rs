#![doc = include_str!("../README.md")]

use std::{
    io::{Read, Write},
    pin::{Pin, pin},
    task::{self, Poll, Waker, ready},
};

use futures::{AsyncBufRead, AsyncRead, AsyncWrite, future::poll_fn, io};

mod half;

pub use half::{ReadGuard, ReadHalf, WriteGuard, WriteHalf};

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
pub fn duplex(max_buf_size: usize) -> (DuplexStream, DuplexStream) {
    let (read_0, write_0) = half::split(SimplexStream::new_unsplit(max_buf_size));
    let (read_1, write_1) = half::split(SimplexStream::new_unsplit(max_buf_size));

    (
        DuplexStream {
            read: read_0,
            write: write_1,
        },
        DuplexStream {
            read: read_1,
            write: write_0,
        },
    )
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
    // Determines if the write side has been closed.
    is_closed: bool,
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
        SimplexStream {
            buffer: vec![0; buf_size].into_boxed_slice(),
            ptr: 0,
            len: 0,
            is_closed: false,
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
        } else if self.is_closed {
            Poll::Ready(Ok(&[]))
        } else {
            self.read_waker = Some(cx.waker().clone());
            Poll::Pending
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
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.remaining_mut();
        if avail == 0 {
            self.write_waker = Some(cx.waker().clone());
            Poll::Pending
        } else {
            Poll::Ready(Ok(self.get_mut()))
        }
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

    fn close_write(&mut self) {
        self.is_closed = true;
        // needs to notify any readers that no more data will come
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
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

    fn poll_close(mut self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<io::Result<()>> {
        self.close_write();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simplex_no_capacity() {
        let mut s = SimplexStream::new_unsplit(0);
        assert!(s.get().is_empty());
        assert!(s.get_mut().is_empty());
    }

    #[test]
    fn test_simplex() {
        let mut s = SimplexStream::new_unsplit(8);
        assert!(s.get().is_empty());
        assert_eq!(s.get_mut().len(), 8);
        assert_eq!(s.get_mut(), &vec![0; 8]);

        s.get_mut().copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);

        // write 2 bytes
        s.advance_mut(2);
        assert_eq!(s.remaining(), 2);
        assert_eq!(s.remaining_mut(), 6);
        assert_eq!(s.get(), &[0, 1]);
        assert_eq!(s.get_mut(), &[2, 3, 4, 5, 6, 7]);

        // read 1 byte
        s.advance(1);
        assert_eq!(s.remaining(), 1);
        // space is reclaimed immediately with ring buffer
        assert_eq!(s.remaining_mut(), 7);

        // write the rest of the bytes
        s.advance_mut(6);
        assert_eq!(s.get(), &[1, 2, 3, 4, 5, 6, 7]);
        // read everything out
        s.advance(7);

        assert!(s.get().is_empty());
        assert_eq!(s.get_mut().len(), 8);
    }

    #[test]
    fn test_read_to() {
        let mut s0 = SimplexStream::new_unsplit(16);
        let mut s1 = SimplexStream::new_unsplit(8);

        s0.advance_mut(8);
        assert_eq!(s0.remaining(), 8);

        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert_eq!(s1.remaining(), 0);
        assert!(s0.poll_read_to(&mut cx, &mut s1).is_ready());
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 8);

        s1.advance(8);
        assert_eq!(s1.remaining(), 0);

        s0.advance_mut(4);
        assert_eq!(s0.remaining(), 4);

        assert!(s0.poll_read_to(&mut cx, &mut s1).is_ready());
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 4);

        s1.advance(4);
        assert_eq!(s1.remaining(), 0);

        // With ring buffer, data wraps around and may need multiple reads
        s0.advance_mut(16);
        assert_eq!(s0.remaining(), 16);

        // Transfer in chunks due to ring buffer wrap-around
        while s0.remaining() > 0 && s1.remaining_mut() > 0 {
            assert!(s0.poll_read_to(&mut cx, &mut s1).is_ready());
        }
        assert_eq!(s1.remaining(), 8);
        s1.advance(8);

        while s0.remaining() > 0 && s1.remaining_mut() > 0 {
            assert!(s0.poll_read_to(&mut cx, &mut s1).is_ready());
        }
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 8);
    }

    #[test]
    fn test_write_from() {
        let mut s0 = SimplexStream::new_unsplit(16);
        let mut s1 = SimplexStream::new_unsplit(8);

        s0.advance_mut(8);
        assert_eq!(s0.remaining(), 8);

        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert_eq!(s1.remaining(), 0);
        assert!(s1.poll_write_from(&mut cx, &mut s0).is_ready());
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 8);

        s1.advance(8);
        assert_eq!(s1.remaining(), 0);

        s0.advance_mut(4);
        assert_eq!(s0.remaining(), 4);

        assert!(s1.poll_write_from(&mut cx, &mut s0).is_ready());
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 4);

        s1.advance(4);
        assert_eq!(s1.remaining(), 0);

        // With ring buffer, data wraps around and may need multiple writes
        s0.advance_mut(16);
        assert_eq!(s0.remaining(), 16);

        // Transfer in chunks due to ring buffer wrap-around
        while s0.remaining() > 0 && s1.remaining_mut() > 0 {
            assert!(s1.poll_write_from(&mut cx, &mut s0).is_ready());
        }
        assert_eq!(s1.remaining(), 8);
        s1.advance(8);

        while s0.remaining() > 0 && s1.remaining_mut() > 0 {
            assert!(s1.poll_write_from(&mut cx, &mut s0).is_ready());
        }
        assert_eq!(s0.remaining(), 0);
        assert_eq!(s1.remaining(), 8);
    }

    #[test]
    fn test_ring_buffer_wrap() {
        let mut s = SimplexStream::new_unsplit(8);

        // Fill buffer completely
        s.get_mut().copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7]);
        s.advance_mut(8);
        assert_eq!(s.remaining(), 8);
        assert_eq!(s.remaining_mut(), 0);

        // Read 4 bytes - this frees space at the front
        assert_eq!(s.get(), &[0, 1, 2, 3, 4, 5, 6, 7]);
        s.advance(4);
        // ptr=4, len=4
        assert_eq!(s.remaining(), 4);
        assert_eq!(s.remaining_mut(), 4);
        assert_eq!(s.get(), &[4, 5, 6, 7]);

        // Write space wraps to the front
        // tail = (4+4) % 8 = 0, so get_mut returns buffer[0..4]
        assert_eq!(s.get_mut().len(), 4);
        s.get_mut().copy_from_slice(&[8, 9, 10, 11]);
        s.advance_mut(4);
        // ptr=4, len=8
        assert_eq!(s.remaining(), 8);
        assert_eq!(s.remaining_mut(), 0);

        // Read wraps around - first chunk is [4,5,6,7]
        assert_eq!(s.get(), &[4, 5, 6, 7]);
        s.advance(4);
        // ptr=0, len=4

        // Second chunk is [8,9,10,11]
        assert_eq!(s.get(), &[8, 9, 10, 11]);
        s.advance(4);
        // ptr=4, len=0

        assert!(s.get().is_empty());
        assert_eq!(s.remaining_mut(), 8);
    }
}
