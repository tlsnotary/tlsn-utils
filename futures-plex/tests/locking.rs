//! Locking and the `DuplexStream` poll-based helpers.
//!
//! `poll_read_to` and `poll_write_from` run user code while holding the lock
//! of one direction, so they are the paths where a blocking `Drop` used to
//! deadlock.

mod common;

use std::{
    io,
    pin::Pin,
    task::{self, Context, Poll, Waker},
    time::Duration,
};

use futures::{AsyncRead, AsyncWrite, io::AsyncWriteExt};
use futures_plex::{DuplexStream, duplex};

use common::run_with_timeout;

/// Drops its peer on the first `poll_write`, standing in for user code that
/// drops a stream while `poll_read_to` is polling it.
struct DropPeerOnWrite(Option<DuplexStream>);

impl AsyncWrite for DropPeerOnWrite {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        drop(self.0.take());
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Accepts the write on the second poll, returning `Pending` on the first.
struct PendingOnce {
    pending: bool,
    written: Vec<u8>,
}

impl AsyncWrite for PendingOnce {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.pending {
            self.pending = false;
            return Poll::Pending;
        }

        self.written.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

/// Always fails to read.
struct FailingReader;

impl AsyncRead for FailingReader {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Err(io::Error::other("boom")))
    }
}

#[test]
fn drop_inside_poll_read_to_does_not_block() {
    run_with_timeout(Duration::from_secs(5), || {
        let (a, mut b) = duplex(64);
        futures::executor::block_on(b.write_all(b"hi")).unwrap();

        let mut sink = DropPeerOnWrite(Some(b));
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Dropping the peer here happens while the read side of `a` is locked.
        match a.poll_read_to(&mut cx, &mut sink) {
            Poll::Ready(Ok(2)) => {}
            other => panic!("expected two bytes to be copied, got {other:?}"),
        }
    });
}

#[test]
fn read_lock_excludes_peer_write() {
    let (a, b) = duplex(8);
    let waker = Waker::noop();
    let mut cx = task::Context::from_waker(waker);

    let guard = match a.poll_lock_read(&mut cx) {
        Poll::Ready(guard) => guard,
        Poll::Pending => panic!("read lock should be free"),
    };
    // The peer's write half is the other side of that same lock.
    assert!(b.poll_lock_write(&mut cx).is_pending());
    drop(guard);
    assert!(matches!(b.poll_lock_write(&mut cx), Poll::Ready(_)));

    // And the other direction.
    let guard = match b.poll_lock_write(&mut cx) {
        Poll::Ready(guard) => guard,
        Poll::Pending => panic!("write lock should be free"),
    };
    assert!(a.poll_lock_read(&mut cx).is_pending());
    drop(guard);
    assert!(matches!(a.poll_lock_read(&mut cx), Poll::Ready(_)));
}

#[test]
fn poll_read_to_resumes_after_pending() {
    let (a, mut b) = duplex(8);
    futures::executor::block_on(b.write_all(b"hello")).unwrap();

    let mut sink = PendingOnce {
        pending: true,
        written: Vec::new(),
    };
    let waker = Waker::noop();
    let mut cx = task::Context::from_waker(waker);

    assert!(a.poll_read_to(&mut cx, &mut sink).is_pending());
    // Nothing may be consumed by the attempt that returned `Pending`.
    assert!(matches!(
        a.poll_read_to(&mut cx, &mut sink),
        Poll::Ready(Ok(5))
    ));
    assert_eq!(sink.written, b"hello");
}

#[test]
fn poll_write_from_propagates_reader_error() {
    let (mut a, mut b) = duplex(8);
    let waker = Waker::noop();
    let mut cx = task::Context::from_waker(waker);

    let err = match a.poll_write_from(&mut cx, &mut FailingReader) {
        Poll::Ready(Err(err)) => err,
        other => panic!("expected the read error to surface, got {other:?}"),
    };
    assert_eq!(err.kind(), io::ErrorKind::Other);

    // The failed attempt must not have written anything.
    assert!(matches!(
        AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"ok"),
        Poll::Ready(Ok(2))
    ));
    let mut buf = [0u8; 2];
    assert!(matches!(
        AsyncRead::poll_read(Pin::new(&mut b), &mut cx, &mut buf),
        Poll::Ready(Ok(2))
    ));
    assert_eq!(&buf, b"ok");
}
