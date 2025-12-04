#![doc = include_str!("../README.md")]

use std::{
    io::{Read, Write},
    pin::Pin,
    task::{self, Poll, Waker},
};

use bytes::{Buf, BytesMut};
use futures_io::{AsyncRead, AsyncWrite};

mod half;
pub use half::{ReadHalf, WriteHalf};

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
/// # use futures_util::{AsyncReadExt, AsyncWriteExt};
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
/// # use futures_util::{AsyncReadExt, AsyncWriteExt};
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
    /// The buffer storing the bytes written, also read from.
    ///
    /// Using a `BytesMut` because it has efficient `Buf` and `BufMut`
    /// functionality already. Additionally, it can try to copy data in the
    /// same buffer if there read index has advanced far enough.
    buffer: BytesMut,
    /// Determines if the write side has been closed.
    is_closed: bool,
    /// The maximum amount of bytes that can be written before returning
    /// `Poll::Pending`.
    max_buf_size: usize,
    /// If the `read` side has been polled and is pending, this is the waker
    /// for that parked task.
    read_waker: Option<Waker>,
    /// If the `write` side has filled the `max_buf_size` and returned
    /// `Poll::Pending`, this is the waker for that parked task.
    write_waker: Option<Waker>,
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
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.read).poll_read(cx, buf)
    }
}

impl AsyncWrite for DuplexStream {
    #[allow(unused_mut)]
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
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
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_flush(cx)
    }

    #[allow(unused_mut)]
    fn poll_close(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.write).poll_close(cx)
    }
}

impl Read for DuplexStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        Read::read(&mut self.read, buf)
    }
}

impl Write for DuplexStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Write::write(&mut self.write, buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Write::flush(&mut self.write)
    }
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
/// # use futures_util::{AsyncReadExt, AsyncWriteExt};
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
    /// The `max_buf_size` argument is the maximum amount of bytes that can be
    /// written to a buffer before the it returns `Poll::Pending`.
    pub fn new_unsplit(max_buf_size: usize) -> SimplexStream {
        SimplexStream {
            buffer: BytesMut::new(),
            is_closed: false,
            max_buf_size,
            read_waker: None,
            write_waker: None,
        }
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
    ) -> Poll<std::io::Result<usize>> {
        if self.buffer.has_remaining() {
            let len = self.buffer.remaining().min(buf.len());
            buf[..len].copy_from_slice(&self.buffer[..len]);
            self.buffer.advance(len);
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

    fn poll_write_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.max_buf_size - self.buffer.len();
        if avail == 0 {
            self.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let len = buf.len().min(avail);
        self.buffer.extend_from_slice(&buf[..len]);
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(len))
    }

    fn poll_write_vectored_internal(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<Result<usize, std::io::Error>> {
        if self.is_closed {
            return Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()));
        }
        let avail = self.max_buf_size - self.buffer.len();
        if avail == 0 {
            self.write_waker = Some(cx.waker().clone());
            return Poll::Pending;
        }

        let mut rem = avail;
        for buf in bufs {
            if rem == 0 {
                break;
            }

            let len = buf.len().min(rem);
            self.buffer.extend_from_slice(&buf[..len]);
            rem -= len;
        }

        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        Poll::Ready(Ok(avail - rem))
    }
}

impl AsyncRead for SimplexStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut [u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_read_internal(cx, buf)
    }
}

impl AsyncWrite for SimplexStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_write_internal(cx, buf)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        self.poll_write_vectored_internal(cx, bufs)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut task::Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(
        mut self: Pin<&mut Self>,
        _: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.close_write();
        Poll::Ready(Ok(()))
    }
}

impl Read for SimplexStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.buffer.has_remaining() {
            let len = self.buffer.remaining().min(buf.len());
            buf[..len].copy_from_slice(&self.buffer[..len]);
            self.buffer.advance(len);
            if len > 0 {
                // The passed `buf` might have been empty, don't wake up if
                // no bytes have been moved.
                if let Some(waker) = self.write_waker.take() {
                    waker.wake();
                }
            }
            Ok(len)
        } else {
            Ok(0)
        }
    }
}

impl Write for SimplexStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.is_closed {
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        let avail = self.max_buf_size - self.buffer.len();
        if avail == 0 {
            return Ok(0);
        }

        let len = buf.len().min(avail);
        self.buffer.extend_from_slice(&buf[..len]);
        if let Some(waker) = self.read_waker.take() {
            waker.wake();
        }
        Ok(len)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
