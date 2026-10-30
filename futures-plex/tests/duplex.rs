//! Peer-drop and close semantics for [`futures_plex::duplex`] streams.

mod common;

use std::{pin::Pin, task};

use futures::{AsyncRead, AsyncWrite};
use futures_plex::duplex;

use common::CountingWaker;

#[test]
fn drop_closes_both_directions() {
    use futures::io::{AsyncReadExt, AsyncWriteExt};

    let (mut a, mut b) = duplex(8);
    futures::executor::block_on(AsyncWriteExt::write_all(&mut b, b"hi")).unwrap();
    drop(b);

    // Buffered data is still readable, then the reader sees EOF.
    let mut buf = [0u8; 2];
    futures::executor::block_on(AsyncReadExt::read_exact(&mut a, &mut buf)).unwrap();
    assert_eq!(&buf, b"hi");
    assert_eq!(
        futures::executor::block_on(AsyncReadExt::read(&mut a, &mut buf)).unwrap(),
        0
    );

    // Writes into the dropped peer fail.
    let err = futures::executor::block_on(AsyncWriteExt::write(&mut a, &[0u8; 1])).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn parked_reader_woken_by_peer_drop() {
    let (mut a, b) = duplex(8);
    let waker = CountingWaker::new();
    let cx = std::task::Waker::from(waker.clone());
    let mut cx = task::Context::from_waker(&cx);
    let mut buf = [0u8; 4];

    assert!(AsyncRead::poll_read(Pin::new(&mut a), &mut cx, &mut buf).is_pending());
    assert_eq!(waker.count(), 0);

    drop(b);
    assert_eq!(waker.count(), 1);
    assert!(matches!(
        AsyncRead::poll_read(Pin::new(&mut a), &mut cx, &mut buf),
        task::Poll::Ready(Ok(0))
    ));
}

#[test]
fn parked_writer_woken_by_peer_drop() {
    let (mut a, b) = duplex(2);
    let waker = CountingWaker::new();
    let cx = std::task::Waker::from(waker.clone());
    let mut cx = task::Context::from_waker(&cx);

    assert!(matches!(
        AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"ab"),
        task::Poll::Ready(Ok(2))
    ));
    assert!(AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"c").is_pending());
    assert_eq!(waker.count(), 0);

    drop(b);
    assert_eq!(waker.count(), 1);

    let err = match AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"c") {
        task::Poll::Ready(Err(err)) => err,
        other => panic!("expected BrokenPipe, got {other:?}"),
    };
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn parked_reader_woken_by_close() {
    let (mut a, mut b) = duplex(8);
    let waker = CountingWaker::new();
    let cx = std::task::Waker::from(waker.clone());
    let mut cx = task::Context::from_waker(&cx);
    let mut buf = [0u8; 4];

    assert!(AsyncRead::poll_read(Pin::new(&mut a), &mut cx, &mut buf).is_pending());
    assert_eq!(waker.count(), 0);

    // Closing the write side of the peer must wake the parked reader.
    assert!(AsyncWrite::poll_close(Pin::new(&mut b), &mut cx).is_ready());
    assert_eq!(waker.count(), 1);
    assert!(matches!(
        AsyncRead::poll_read(Pin::new(&mut a), &mut cx, &mut buf),
        task::Poll::Ready(Ok(0))
    ));
}

#[test]
fn drop_other_side_closes_both_directions() {
    use futures::io::{AsyncReadExt, AsyncWriteExt};

    // Mirror of `drop_closes_both_directions`, dropping the reading side.
    let (a, mut b) = duplex(8);
    futures::executor::block_on(AsyncWriteExt::write_all(&mut b, b"hi")).unwrap();
    drop(a);

    let mut buf = [0u8; 2];
    assert_eq!(
        futures::executor::block_on(AsyncReadExt::read(&mut b, &mut buf)).unwrap(),
        0
    );

    let err = futures::executor::block_on(AsyncWriteExt::write(&mut b, b"x")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn drop_preserves_wrapped_buffered_data() {
    use futures::io::{AsyncReadExt, AsyncWriteExt};

    let (mut a, mut b) = duplex(8);
    futures::executor::block_on(AsyncWriteExt::write_all(&mut b, b"01234567")).unwrap();

    let mut head = [0u8; 4];
    futures::executor::block_on(AsyncReadExt::read_exact(&mut a, &mut head)).unwrap();
    assert_eq!(&head, b"0123");

    // This write wraps to the front of the ring buffer.
    futures::executor::block_on(AsyncWriteExt::write_all(&mut b, b"89ab")).unwrap();
    drop(b);

    // Closing must not discard what is still buffered, and the order must
    // survive the wrap.
    let mut rest = Vec::new();
    futures::executor::block_on(AsyncReadExt::read_to_end(&mut a, &mut rest)).unwrap();
    assert_eq!(rest, b"456789ab");
}

#[test]
fn drop_after_poll_close_is_idempotent() {
    use futures::io::{AsyncReadExt, AsyncWriteExt};

    let (mut a, mut b) = duplex(8);
    futures::executor::block_on(AsyncWriteExt::close(&mut b)).unwrap();
    // Closing again (via drop) must not panic.
    drop(b);

    let mut buf = [0u8; 1];
    assert_eq!(
        futures::executor::block_on(AsyncReadExt::read(&mut a, &mut buf)).unwrap(),
        0
    );

    let err = futures::executor::block_on(AsyncWriteExt::write(&mut a, b"x")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::BrokenPipe);
}

#[test]
fn eof_is_sticky() {
    use futures::io::AsyncReadExt;

    let (mut a, b) = duplex(8);
    drop(b);

    let mut buf = [0u8; 4];
    for _ in 0..3 {
        assert_eq!(
            futures::executor::block_on(AsyncReadExt::read(&mut a, &mut buf)).unwrap(),
            0
        );
    }
}

#[test]
#[should_panic(expected = "max_buf_size")]
fn duplex_rejects_zero_capacity() {
    let _ = duplex(0);
}
