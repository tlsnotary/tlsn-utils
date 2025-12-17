//! Adapted from <https://github.com/rust-lang/futures-rs/blob/master/futures-util/src/io/split.rs>
//! to support sync operations.

use futures::{
    AsyncRead, AsyncWrite,
    io::{IoSlice, IoSliceMut},
    lock::{BiLock, BiLockGuard},
};

use std::{
    fmt,
    io::{self},
    ops::{Deref, DerefMut},
    pin::Pin,
    task::{Context, Poll, ready},
};

use crate::SimplexStream;

#[derive(Debug)]
pub struct ReadGuard<'a, T>(pub(crate) BiLockGuard<'a, T>);

impl<'a, T> Deref for ReadGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &(*self.0)
    }
}

impl<'a, T: Unpin> DerefMut for ReadGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_pin_mut().get_mut()
    }
}

/// The readable half of an object.
#[derive(Debug)]
pub struct ReadHalf<T> {
    pub(crate) handle: BiLock<T>,
}

#[derive(Debug)]
pub struct WriteGuard<'a, T>(pub(crate) BiLockGuard<'a, T>);

impl<'a, T> Deref for WriteGuard<'a, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &(*self.0)
    }
}

impl<'a, T: Unpin> DerefMut for WriteGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_pin_mut().get_mut()
    }
}

/// The writable half of an object.
#[derive(Debug)]
pub struct WriteHalf<T> {
    handle: BiLock<T>,
}

fn lock_and_then<T, U, E, F>(lock: &BiLock<T>, cx: &mut Context<'_>, f: F) -> Poll<Result<U, E>>
where
    F: FnOnce(Pin<&mut T>, &mut Context<'_>) -> Poll<Result<U, E>>,
{
    let mut l = ready!(lock.poll_lock(cx));
    f(l.as_pin_mut(), cx)
}

pub(crate) fn split<T: AsyncRead + AsyncWrite>(t: T) -> (ReadHalf<T>, WriteHalf<T>) {
    let (a, b) = BiLock::new(t);
    (ReadHalf { handle: a }, WriteHalf { handle: b })
}

impl<T> ReadHalf<T> {
    /// Attempt to acquire a lock on the read side, returning `Poll::Pending` if
    /// it can't be acquired.
    pub fn poll_lock(&self, cx: &mut Context<'_>) -> Poll<ReadGuard<'_, T>> {
        self.handle.poll_lock(cx).map(ReadGuard)
    }

    /// Checks if this `ReadHalf` and some `WriteHalf` were split from the same
    /// stream.
    pub fn is_pair_of(&self, other: &WriteHalf<T>) -> bool {
        self.handle.is_pair_of(&other.handle)
    }
}

impl<T: Unpin> ReadHalf<T> {
    /// Attempts to put the two "halves" of a split `AsyncRead + AsyncWrite`
    /// back together. Succeeds only if the `ReadHalf<T>` and `WriteHalf<T>`
    /// are a matching pair originating from the same call to `split`.
    pub fn reunite(self, other: WriteHalf<T>) -> Result<T, ReuniteError<T>> {
        self.handle
            .reunite(other.handle)
            .map_err(|err| ReuniteError(Self { handle: err.0 }, WriteHalf { handle: err.1 }))
    }
}

impl<T> WriteHalf<T> {
    /// Attempt to acquire a lock on the write side, returning `Poll::Pending`
    /// if it can't be acquired.
    pub fn poll_lock(&self, cx: &mut Context<'_>) -> Poll<WriteGuard<'_, T>> {
        self.handle.poll_lock(cx).map(WriteGuard)
    }

    /// Checks if this `WriteHalf` and some `ReadHalf` were split from the same
    /// stream.
    pub fn is_pair_of(&self, other: &ReadHalf<T>) -> bool {
        self.handle.is_pair_of(&other.handle)
    }
}

impl<T: Unpin> WriteHalf<T> {
    /// Attempts to put the two "halves" of a split `AsyncRead + AsyncWrite`
    /// back together. Succeeds only if the `ReadHalf<T>` and `WriteHalf<T>`
    /// are a matching pair originating from the same call to `split`.
    pub fn reunite(self, other: ReadHalf<T>) -> Result<T, ReuniteError<T>> {
        other.reunite(self)
    }
}

impl<R: AsyncRead> AsyncRead for ReadHalf<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_read(cx, buf))
    }

    fn poll_read_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
    ) -> Poll<io::Result<usize>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_read_vectored(cx, bufs))
    }
}

impl<W: AsyncWrite> AsyncWrite for WriteHalf<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_write(cx, buf))
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_write_vectored(cx, bufs))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_flush(cx))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        lock_and_then(&self.handle, cx, |l, cx| l.poll_close(cx))
    }
}

impl ReadHalf<SimplexStream> {
    /// Returns the number of bytes that can be read.
    pub fn remaining(&self) -> usize {
        let handle = futures::executor::block_on(self.handle.lock());
        handle.remaining()
    }
}

impl WriteHalf<SimplexStream> {
    /// Returns the number of bytes that can be written.
    pub fn remaining_mut(&self) -> usize {
        let handle = futures::executor::block_on(self.handle.lock());
        handle.remaining_mut()
    }
}

/// Error indicating a `ReadHalf<T>` and `WriteHalf<T>` were not two halves
/// of a `AsyncRead + AsyncWrite`, and thus could not be `reunite`d.
pub struct ReuniteError<T>(pub ReadHalf<T>, pub WriteHalf<T>);

impl<T> fmt::Debug for ReuniteError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ReuniteError").field(&"...").finish()
    }
}

impl<T> fmt::Display for ReuniteError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "tried to reunite a ReadHalf and WriteHalf that don't form a pair"
        )
    }
}

impl<T: core::any::Any> std::error::Error for ReuniteError<T> {}
