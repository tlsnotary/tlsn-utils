//! Backpressure: payloads larger than the buffer, partial writes and the
//! ring-buffer wrap.

mod common;

use std::{
    pin::Pin,
    task::{Context, Poll, Waker},
    time::Duration,
};

use futures::{
    AsyncRead, AsyncWrite,
    io::{AsyncReadExt, AsyncWriteExt},
};
use futures_plex::duplex;

use common::run_with_timeout;

#[test]
fn write_all_larger_than_buffer() {
    run_with_timeout(Duration::from_secs(5), || {
        let (mut a, mut b) = duplex(4);
        let payload: Vec<u8> = (0..12).collect();
        let mut out = vec![0u8; payload.len()];

        // Polling both sides in one task makes the interleaving deterministic:
        // the writer fills the buffer, parks, and the reader drains it.
        let (write, read) = futures::executor::block_on(futures::future::join(
            AsyncWriteExt::write_all(&mut a, &payload),
            AsyncReadExt::read_exact(&mut b, &mut out),
        ));
        write.unwrap();
        read.unwrap();

        assert_eq!(out, payload);
    });
}

#[test]
fn poll_write_returns_partial_count() {
    let (mut a, mut b) = duplex(4);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);

    assert!(matches!(
        AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"ab"),
        Poll::Ready(Ok(2))
    ));
    // Only two bytes of room are left, so four bytes are accepted partially.
    assert!(matches!(
        AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"cdef"),
        Poll::Ready(Ok(2))
    ));
    assert!(AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"ef").is_pending());

    // Nothing was lost or duplicated: the reader sees exactly what fit.
    let mut out = [0u8; 4];
    assert!(matches!(
        AsyncRead::poll_read(Pin::new(&mut b), &mut cx, &mut out),
        Poll::Ready(Ok(4))
    ));
    assert_eq!(&out, b"abcd");

    // Space is available again.
    assert!(matches!(
        AsyncWrite::poll_write(Pin::new(&mut a), &mut cx, b"ef"),
        Poll::Ready(Ok(2))
    ));
}

#[test]
fn read_exact_across_wrap_around() {
    run_with_timeout(Duration::from_secs(5), || {
        let (mut a, mut b) = duplex(8);

        // Fill the buffer, then drain part of it so the next write has to wrap.
        futures::executor::block_on(AsyncWriteExt::write_all(&mut a, &[1u8; 8])).unwrap();
        let mut head = [0u8; 3];
        futures::executor::block_on(AsyncReadExt::read_exact(&mut b, &mut head)).unwrap();
        assert_eq!(head, [1u8; 3]);

        // Five more bytes, physically written at the front of the ring, must
        // still be read after the five bytes left over from the first write.
        let mut tail = [0u8; 10];
        let (write, read) = futures::executor::block_on(futures::future::join(
            AsyncWriteExt::write_all(&mut a, &[2u8; 5]),
            AsyncReadExt::read_exact(&mut b, &mut tail),
        ));
        write.unwrap();
        read.unwrap();

        let mut expected = [0u8; 10];
        expected[..5].fill(1);
        expected[5..].fill(2);
        assert_eq!(tail, expected);
    });
}
