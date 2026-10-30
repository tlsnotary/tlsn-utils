//! Buffer mechanics and wakeups for the unidirectional [`SimplexStream`].

mod common;

use std::task::{self, Poll, Waker};

use futures_plex::SimplexStream;

use common::CountingWaker;

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

#[test]
fn parked_reader_woken_by_write() {
    let mut s = SimplexStream::new_unsplit(8);
    let waker = CountingWaker::new();
    let cx = Waker::from(waker.clone());
    let mut cx = task::Context::from_waker(&cx);

    assert!(s.poll_get(&mut cx).is_pending());
    assert_eq!(waker.count(), 0);

    // Making data available must wake the parked reader.
    s.get_mut()[0] = b'x';
    s.advance_mut(1);
    assert_eq!(waker.count(), 1);

    match s.poll_get(&mut cx) {
        Poll::Ready(Ok(buf)) => assert_eq!(buf, b"x"),
        other => panic!("expected buffered data, got {other:?}"),
    }
}

#[test]
fn parked_writer_woken_by_read() {
    let mut s = SimplexStream::new_unsplit(2);
    s.get_mut()[..2].copy_from_slice(b"ab");
    s.advance_mut(2);

    let waker = CountingWaker::new();
    let cx = Waker::from(waker.clone());
    let mut cx = task::Context::from_waker(&cx);

    assert!(s.poll_mut(&mut cx).is_pending());
    assert_eq!(waker.count(), 0);

    // Draining the buffer must wake the parked writer.
    s.advance(1);
    assert_eq!(waker.count(), 1);
    assert!(s.poll_mut(&mut cx).is_ready());
}

#[test]
fn re_parking_wakes_only_the_latest_waker() {
    let mut s = SimplexStream::new_unsplit(8);
    let first = CountingWaker::new();
    let second = CountingWaker::new();
    let first_waker = Waker::from(first.clone());
    let second_waker = Waker::from(second.clone());

    let mut cx = task::Context::from_waker(&first_waker);
    assert!(s.poll_get(&mut cx).is_pending());
    let mut cx = task::Context::from_waker(&second_waker);
    assert!(s.poll_get(&mut cx).is_pending());

    s.get_mut()[0] = b'x';
    s.advance_mut(1);

    assert_eq!(first.count(), 0);
    assert_eq!(second.count(), 1);
}
